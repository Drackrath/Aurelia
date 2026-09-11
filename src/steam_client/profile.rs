//! User profiles over the CM: link details, profile info, persona.
use super::*;
use steam_vent_proto::steammessages_clientserver_friends::{
    CMsgClientFriendProfileInfo, CMsgClientFriendProfileInfoResponse, CMsgClientPersonaState,
};
use steam_vent_proto::steammessages_player_steamclient::{
    CPlayer_GetPlayerLinkDetails_Request, CPlayer_GetPlayerLinkDetails_Response,
};
use tokio_stream::StreamExt;

/// `EAccountFlags::LimitedUser`.
const ACCOUNT_FLAG_LIMITED: u32 = 4096;
/// Community visibility: 1 private, 2 friends-only, 3 public.
const VISIBILITY_PUBLIC: i32 = 3;

/// Everything the CM tells us about one user.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UserProfile {
    pub steam_id: u64,
    pub persona_name: Option<String>,
    pub profile_url: String,
    pub avatar_url: Option<String>,
    /// 1 private, 2 friends-only, 3 public.
    pub visibility: i32,
    /// Raw `privacy_state` as sent by Steam.
    pub privacy_state: i32,
    pub is_public: bool,
    pub is_limited: bool,
    pub ban_expires: Option<u64>,
    pub created: Option<u64>,
    pub real_name: Option<String>,
    pub location: Option<String>,
    pub headline: Option<String>,
    pub summary: Option<String>,
    /// 0 offline … 6 looking-to-play.
    pub persona_state: Option<u32>,
    pub game_app_id: Option<u32>,
    pub game_name: Option<String>,
    pub last_logoff: Option<u64>,
    /// Which sources answered.
    pub sources: Vec<String>,
}

fn nonempty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn avatar_url(hash: &[u8]) -> Option<String> {
    if hash.is_empty() || hash.iter().all(|&b| b == 0) {
        return None;
    }
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("https://avatars.cloudflare.steamstatic.com/{hex}_full.jpg"))
}

impl SteamClient {
    /// Merge link details, profile info and persona state.
    pub async fn user_profile(&self, steam_id: u64) -> Result<UserProfile> {
        let connection = self.require_connection()?;
        let mut profile = UserProfile {
            steam_id,
            profile_url: format!("https://steamcommunity.com/profiles/{steam_id}"),
            ..Default::default()
        };

        // 1. Public link details (works for anyone).
        let mut link = CPlayer_GetPlayerLinkDetails_Request::new();
        link.steamids.push(steam_id);
        match connection
            .service_method::<CPlayer_GetPlayerLinkDetails_Request>(link)
            .await
        {
            Ok(resp) => {
                let resp: CPlayer_GetPlayerLinkDetails_Response = resp;
                if let Some(acc) = resp.accounts.first() {
                    if let Some(p) = acc.public_data.as_ref() {
                        profile.persona_name = nonempty(p.persona_name());
                        // `profile_url` is the custom-URL slug, not a URL.
                        if let Some(slug) = nonempty(p.profile_url()) {
                            profile.profile_url = if slug.starts_with("http") {
                                slug
                            } else {
                                format!("https://steamcommunity.com/id/{slug}")
                            };
                        }
                        profile.avatar_url = avatar_url(p.sha_digest_avatar());
                        profile.privacy_state = p.privacy_state();
                        profile.visibility = p.visibility_state();
                        profile.is_public = p.visibility_state() == VISIBILITY_PUBLIC;
                        profile.is_limited = p.account_flags() & ACCOUNT_FLAG_LIMITED != 0;
                        profile.ban_expires = (p.ban_expires_time() > 0).then(|| u64::from(p.ban_expires_time()));
                    }
                    if let Some(p) = acc.private_data.as_ref() {
                        profile.persona_state = Some(p.persona_state().max(0) as u32);
                        profile.game_app_id = (p.game_id() > 0).then(|| p.game_id() as u32);
                        profile.game_name = nonempty(p.game_extra_info());
                        profile.last_logoff = (p.last_logoff_time() > 0).then(|| u64::from(p.last_logoff_time()));
                        profile.created = (p.time_created() > 0).then(|| u64::from(p.time_created()));
                    }
                    profile.sources.push("link_details".to_string());
                }
            }
            Err(e) => tracing::debug!("GetPlayerLinkDetails failed: {e}"),
        }

        // 2. Profile text (real name, location, summary).
        let mut info = CMsgClientFriendProfileInfo::new();
        info.set_steamid_friend(steam_id);
        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.job::<_, CMsgClientFriendProfileInfoResponse>(info),
        )
        .await
        {
            Ok(Ok(resp)) if resp.eresult() == 1 => {
                profile.real_name = nonempty(resp.real_name());
                let place: Vec<String> = [resp.city_name(), resp.state_name(), resp.country_name()]
                    .into_iter()
                    .filter_map(nonempty)
                    .collect();
                profile.location = (!place.is_empty()).then(|| place.join(", "));
                profile.headline = nonempty(resp.headline());
                profile.summary = nonempty(resp.summary());
                if profile.created.is_none() && resp.time_created() > 0 {
                    profile.created = Some(u64::from(resp.time_created()));
                }
                profile.sources.push("profile_info".to_string());
            }
            Ok(Ok(resp)) => tracing::debug!("ClientFriendProfileInfo EResult {}", resp.eresult()),
            Ok(Err(e)) => tracing::debug!("ClientFriendProfileInfo failed: {e}"),
            Err(_) => tracing::debug!("ClientFriendProfileInfo timed out"),
        }

        // 3. Live persona state (name, status, game).
        let mut stream = connection.on::<CMsgClientPersonaState>();
        if let Err(e) = self.announce_configured_presence().await {
            tracing::debug!("persona announce failed: {e}");
        }
        if self.request_friend_data(&[steam_id]).await.is_ok() {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, stream.next()).await {
                    Ok(Some(Ok(state))) => {
                        if let Some(fr) = state.friends.iter().find(|f| f.friendid() == steam_id) {
                            if let Some(name) = nonempty(fr.player_name()) {
                                profile.persona_name = Some(name);
                            }
                            if fr.has_persona_state() {
                                profile.persona_state = Some(fr.persona_state());
                            }
                            if fr.game_played_app_id() > 0 {
                                profile.game_app_id = Some(fr.game_played_app_id());
                            }
                            if let Some(g) = nonempty(fr.game_name()) {
                                profile.game_name = Some(g);
                            }
                            if fr.last_logoff() > 0 {
                                profile.last_logoff = Some(u64::from(fr.last_logoff()));
                            }
                            if profile.avatar_url.is_none() {
                                profile.avatar_url = avatar_url(fr.avatar_hash());
                            }
                            profile.sources.push("persona_state".to_string());
                            break;
                        }
                    }
                    Ok(Some(Err(_))) => continue,
                    _ => break,
                }
            }
        }

        if profile.sources.is_empty() {
            return Err(crate::core::error::TypedError::new(
                crate::core::error::ErrorKind::NotFound,
                format!("Steam returned no profile data for {steam_id}"),
            )
            .into());
        }
        Ok(profile)
    }
}
