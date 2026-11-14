//! Global stats, achievement rarity, leaderboards.
//!
//! Leaderboards use the CM `ClientLBS*` jobs.

use super::rich_presence::AppRouted;
use super::*;
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgClientUGSGetGlobalStats, CMsgClientUGSGetGlobalStatsResponse,
};
use steam_vent_proto::steammessages_clientserver_lbs::{
    CMsgClientLBSFindOrCreateLB, CMsgClientLBSFindOrCreateLBResponse, CMsgClientLBSGetLBEntries,
    CMsgClientLBSGetLBEntriesResponse, CMsgClientLBSSetScore, CMsgClientLBSSetScoreResponse,
};

/// Most history days Steam keeps.
pub const MAX_HISTORY_DAYS: u32 = 60;

/// Most detail ints per entry.
pub const MAX_DETAILS: usize = 64;

/// One day of aggregated stats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalDay {
    pub day_id: u32,
    /// Stat ID and raw value.
    pub stats: Vec<(u32, i64)>,
}

/// Steam's global stats answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalStats {
    pub eresult: i32,
    pub day_current: u32,
    pub days: Vec<GlobalDay>,
}

/// Achievement rarity and icon files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AchievementInfo {
    pub name: String,
    /// Percent of players, 0 to 100.
    pub percent: f32,
    /// Icon URLs, empty when unset.
    pub icon: String,
    pub icon_gray: String,
}

/// A found leaderboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Leaderboard {
    pub eresult: i32,
    pub id: i32,
    pub entries: i32,
    pub sort: i32,
    pub display: i32,
    pub name: String,
}

/// One leaderboard row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaderboardEntry {
    pub steam_id: u64,
    pub rank: i32,
    pub score: i32,
    pub ugc: u64,
    /// Little-endian `int32` details.
    pub details: Vec<u8>,
}

/// Downloaded rows and board size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaderboardEntries {
    pub eresult: i32,
    pub total: i32,
    pub entries: Vec<LeaderboardEntry>,
}

/// What to download.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntriesRequest {
    pub leaderboard: i32,
    /// `ELeaderboardDataRequest` value.
    pub request: i32,
    pub start: i32,
    pub end: i32,
    pub users: Vec<u64>,
}

/// Steam's answer to an upload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScoreUpload {
    pub eresult: i32,
    pub entries: i32,
    pub changed: bool,
    pub rank_new: i32,
    pub rank_previous: i32,
}

/// Steam's percent string, clamped.
pub fn parse_percent(text: &str) -> f32 {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|p| p.is_finite())
        .map_or(0.0, |p| p.clamp(0.0, 100.0))
}

/// Details truncated to Steam's limit.
pub fn clamp_details(details: &[u8]) -> Vec<u8> {
    let n = details.len() / 4 * 4;
    details[..n.min(MAX_DETAILS * 4)].to_vec()
}

pub fn global_stats_from(response: &CMsgClientUGSGetGlobalStatsResponse) -> GlobalStats {
    GlobalStats {
        eresult: response.eresult(),
        day_current: u32::try_from(response.day_current()).unwrap_or(0),
        days: response
            .days
            .iter()
            .map(|d| GlobalDay {
                day_id: d.day_id(),
                stats: d
                    .stats
                    .iter()
                    .filter_map(|s| Some((u32::try_from(s.stat_id()).ok()?, s.data())))
                    .collect(),
            })
            .collect(),
    }
}

pub fn entries_from(response: &CMsgClientLBSGetLBEntriesResponse) -> LeaderboardEntries {
    LeaderboardEntries {
        eresult: response.eresult(),
        total: response.leaderboard_entry_count(),
        entries: response
            .entries
            .iter()
            .map(|e| LeaderboardEntry {
                steam_id: e.steam_id_user(),
                rank: e.global_rank(),
                score: e.score(),
                ugc: e.ugc_id(),
                details: clamp_details(e.details()),
            })
            .collect(),
    }
}

impl SteamClient {
    /// Aggregated stats with daily history.
    pub async fn global_stats(&self, appid: u32, history_days: u32) -> Result<GlobalStats> {
        let connection = self.require_connection()?;
        let mut msg = CMsgClientUGSGetGlobalStats::new();
        msg.set_gameid(u64::from(appid));
        msg.set_history_days_requested(history_days.min(MAX_HISTORY_DAYS));
        let response: CMsgClientUGSGetGlobalStatsResponse = connection
            .job(AppRouted { msg, app_id: appid })
            .await
            .context("ClientUGSGetGlobalStats failed")?;
        Ok(global_stats_from(&response))
    }

    /// Rarity and icons of every achievement.
    pub async fn achievement_info(
        &self,
        appid: u32,
        language: &str,
    ) -> Result<Vec<AchievementInfo>> {
        let connection = self.require_connection()?;
        let mut request = CPlayer_GetGameAchievements_Request::new();
        request.set_appid(appid);
        request.set_language(language.to_string());
        let response: CPlayer_GetGameAchievements_Response = connection
            .service_method(request)
            .await
            .context("Player.GetGameAchievements failed")?;
        Ok(response
            .achievements
            .iter()
            .map(|a| AchievementInfo {
                name: a.internal_name().to_string(),
                percent: parse_percent(a.player_percent_unlocked()),
                icon: achievement_icon_url(appid, a.icon()),
                icon_gray: achievement_icon_url(appid, a.icon_gray()),
            })
            .collect())
    }

    /// Finds, or creates with `create`.
    pub async fn find_leaderboard(
        &self,
        appid: u32,
        name: &str,
        create: Option<(i32, i32)>,
    ) -> Result<Leaderboard> {
        let connection = self.require_connection()?;
        let mut msg = CMsgClientLBSFindOrCreateLB::new();
        msg.set_app_id(appid);
        msg.set_leaderboard_name(name.to_string());
        msg.set_create_if_not_found(create.is_some());
        if let Some((sort, display)) = create {
            msg.set_leaderboard_sort_method(sort);
            msg.set_leaderboard_display_type(display);
        }
        let response: CMsgClientLBSFindOrCreateLBResponse = connection
            .job(AppRouted { msg, app_id: appid })
            .await
            .context("ClientLBSFindOrCreateLB failed")?;
        Ok(Leaderboard {
            eresult: response.eresult(),
            id: response.leaderboard_id(),
            entries: response.leaderboard_entry_count(),
            sort: response.leaderboard_sort_method(),
            display: response.leaderboard_display_type(),
            name: response.leaderboard_name().to_string(),
        })
    }

    /// Downloads leaderboard rows.
    pub async fn leaderboard_entries(
        &self,
        appid: u32,
        request: &EntriesRequest,
    ) -> Result<LeaderboardEntries> {
        let connection = self.require_connection()?;
        let mut msg = CMsgClientLBSGetLBEntries::new();
        msg.set_app_id(i32::try_from(appid).context("app ID out of range")?);
        msg.set_leaderboard_id(request.leaderboard);
        msg.set_leaderboard_data_request(request.request);
        msg.set_range_start(request.start);
        msg.set_range_end(request.end);
        msg.steamids = request.users.clone();
        let response: CMsgClientLBSGetLBEntriesResponse = connection
            .job(AppRouted { msg, app_id: appid })
            .await
            .context("ClientLBSGetLBEntries failed")?;
        Ok(entries_from(&response))
    }

    /// Uploads our score; a write.
    pub async fn upload_leaderboard_score(
        &self,
        appid: u32,
        leaderboard: i32,
        method: i32,
        score: i32,
        details: &[u8],
    ) -> Result<ScoreUpload> {
        let connection = self.require_connection()?;
        let mut msg = CMsgClientLBSSetScore::new();
        msg.set_app_id(appid);
        msg.set_leaderboard_id(leaderboard);
        msg.set_score(score);
        msg.set_details(clamp_details(details));
        msg.set_upload_score_method(method);
        let response: CMsgClientLBSSetScoreResponse = connection
            .job(AppRouted { msg, app_id: appid })
            .await
            .context("ClientLBSSetScore failed")?;
        Ok(ScoreUpload {
            eresult: response.eresult(),
            entries: response.leaderboard_entry_count(),
            changed: response.score_changed(),
            rank_new: response.global_rank_new(),
            rank_previous: response.global_rank_previous(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_vent_proto::steammessages_clientserver_2::cmsg_client_ugsget_global_stats_response;
    use steam_vent_proto::steammessages_clientserver_lbs::cmsg_client_lbsget_lbentries_response;

    #[test]
    fn percent_strings() {
        assert_eq!(parse_percent("12.5"), 12.5);
        assert_eq!(parse_percent(" 3 "), 3.0);
        assert_eq!(parse_percent(""), 0.0);
        assert_eq!(parse_percent("NaN"), 0.0);
        assert_eq!(parse_percent("250"), 100.0);
        assert_eq!(parse_percent("-1"), 0.0);
    }

    #[test]
    fn details_keep_whole_ints() {
        assert_eq!(clamp_details(&[1, 0, 0, 0, 9]), vec![1, 0, 0, 0]);
        assert_eq!(clamp_details(&[0; 300]).len(), MAX_DETAILS * 4);
    }

    #[test]
    fn global_stats_keep_days() {
        use cmsg_client_ugsget_global_stats_response::{Day, day::Stat};
        let mut response = CMsgClientUGSGetGlobalStatsResponse::new();
        response.set_eresult(1);
        response.set_day_current(20);
        let mut day = Day::new();
        day.set_day_id(20);
        let mut stat = Stat::new();
        stat.set_stat_id(3);
        stat.set_data(-9);
        day.stats.push(stat);
        let mut bad = Stat::new();
        bad.set_stat_id(-1);
        day.stats.push(bad);
        response.days.push(day);
        let stats = global_stats_from(&response);
        assert_eq!(
            stats,
            GlobalStats {
                eresult: 1,
                day_current: 20,
                days: vec![GlobalDay {
                    day_id: 20,
                    stats: vec![(3, -9)]
                }],
            }
        );
    }

    #[test]
    fn entries_map_rows() {
        let mut response = CMsgClientLBSGetLBEntriesResponse::new();
        response.set_eresult(1);
        response.set_leaderboard_entry_count(40);
        let mut entry = cmsg_client_lbsget_lbentries_response::Entry::new();
        entry.set_steam_id_user(7);
        entry.set_global_rank(2);
        entry.set_score(300);
        entry.set_details(vec![5, 0, 0, 0, 1]);
        entry.set_ugc_id(11);
        response.entries.push(entry);
        let got = entries_from(&response);
        assert_eq!((got.eresult, got.total), (1, 40));
        assert_eq!(
            got.entries,
            vec![LeaderboardEntry {
                steam_id: 7,
                rank: 2,
                score: 300,
                ugc: 11,
                details: vec![5, 0, 0, 0],
            }]
        );
    }
}
