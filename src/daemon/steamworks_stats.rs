//! Global stats and leaderboards, wire side.

use aurelia::steam_client::global_stats::{
    AchievementInfo, EntriesRequest, GlobalStats, Leaderboard, LeaderboardEntries, ScoreUpload,
};

use super::steamworks::{Fields, put_blob};

/// Stats range 30-39, continued.
pub mod cmd {
    pub const GLOBAL_STATS: u8 = 32;
    pub const ACHIEVEMENT_PERCENTS: u8 = 33;
    pub const LEADERBOARD_FIND: u8 = 34;
    pub const LEADERBOARD_ENTRIES: u8 = 35;
    pub const LEADERBOARD_UPLOAD: u8 = 36;
    pub const ACHIEVEMENT_ICON: u8 = 37;

    pub fn is_global(command: u8) -> bool {
        (GLOBAL_STATS..=ACHIEVEMENT_ICON).contains(&command)
    }
}

/// `DownloadLeaderboardEntriesForUsers` limit.
pub const MAX_USERS: usize = 100;

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_len(out: &mut Vec<u8>, n: usize) {
    put_u32(out, u32::try_from(n).unwrap_or(u32::MAX));
}

/// Eresult, current day, days.
pub fn global_stats_body(stats: &GlobalStats) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, stats.eresult);
    put_u32(&mut out, stats.day_current);
    put_len(&mut out, stats.days.len());
    for day in &stats.days {
        put_u32(&mut out, day.day_id);
        put_len(&mut out, day.stats.len());
        for (id, data) in &day.stats {
            put_u32(&mut out, *id);
            out.extend_from_slice(&data.to_le_bytes());
        }
    }
    out
}

/// Names with percent bits.
pub fn percents_body(infos: &[AchievementInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    put_len(&mut out, infos.len());
    for info in infos {
        put_blob(&mut out, info.name.as_bytes());
        put_u32(&mut out, info.percent.to_bits());
    }
    out
}

/// Find request: create, sort, display, name.
pub fn parse_find(body: &[u8]) -> Option<(bool, i32, i32, String)> {
    let mut f = Fields::new(body);
    let create = f.u8()? != 0;
    let (sort, display) = (f.i32()?, f.i32()?);
    let name = std::str::from_utf8(f.blob()?).ok()?.to_string();
    let valid = !name.is_empty() && name.len() < 128;
    (valid && f.rest().is_empty()).then_some((create, sort, display, name))
}

pub fn leaderboard_body(board: &Leaderboard) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [
        board.eresult,
        board.id,
        board.entries,
        board.sort,
        board.display,
    ] {
        put_i32(&mut out, v);
    }
    put_blob(&mut out, board.name.as_bytes());
    out
}

/// Board, request, range, users.
pub fn parse_entries(body: &[u8]) -> Option<EntriesRequest> {
    let mut f = Fields::new(body);
    let mut request = EntriesRequest {
        leaderboard: f.i32()?,
        request: f.i32()?,
        start: f.i32()?,
        end: f.i32()?,
        users: Vec::new(),
    };
    let count = usize::try_from(f.u32()?).ok()?;
    if count > MAX_USERS {
        return None;
    }
    for _ in 0..count {
        request.users.push(f.u64()?);
    }
    f.rest().is_empty().then_some(request)
}

pub fn entries_body(entries: &LeaderboardEntries) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, entries.eresult);
    put_i32(&mut out, entries.total);
    put_len(&mut out, entries.entries.len());
    for e in &entries.entries {
        out.extend_from_slice(&e.steam_id.to_le_bytes());
        put_i32(&mut out, e.rank);
        put_i32(&mut out, e.score);
        out.extend_from_slice(&e.ugc.to_le_bytes());
        put_blob(&mut out, &e.details);
    }
    out
}

/// Upload: board, method, score, details.
pub fn parse_upload(body: &[u8]) -> Option<(i32, i32, i32, Vec<u8>)> {
    let mut f = Fields::new(body);
    let (board, method, score) = (f.i32()?, f.i32()?, f.i32()?);
    let details = f.blob()?.to_vec();
    (f.rest().is_empty()).then_some((board, method, score, details))
}

pub fn upload_body(upload: &ScoreUpload) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, upload.eresult);
    put_i32(&mut out, upload.entries);
    out.push(u8::from(upload.changed));
    put_i32(&mut out, upload.rank_new);
    put_i32(&mut out, upload.rank_previous);
    out
}

/// Icon request: achieved, name.
pub fn parse_icon(body: &[u8]) -> Option<(bool, String)> {
    let mut f = Fields::new(body);
    let achieved = f.u8()? != 0;
    let name = std::str::from_utf8(f.blob()?).ok()?.to_string();
    (!name.is_empty() && f.rest().is_empty()).then_some((achieved, name))
}
