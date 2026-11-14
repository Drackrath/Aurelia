//! `join`: follow a friend in.

use crate::commands::common::*;
use crate::commands::launch::{PlayTarget, PlayWait, cmd_play};
use crate::daemon;
use crate::daemon::JoinTarget;

use anyhow::{Result, bail};
use aurelia::steam_client::Friend;

/// `aurelia join` options.
pub(crate) struct JoinArgs {
    pub friend: String,
    pub connect: Option<String>,
    pub lobby: Option<u64>,
    pub app: Option<u32>,
    pub no_launch: bool,
}

/// Picks app and target; presence last.
pub(crate) fn plan(
    friend: Option<&Friend>,
    args: &JoinArgs,
    presence_connect: Option<String>,
) -> Result<(u32, JoinTarget)> {
    let Some(app) = args.app.or(friend.and_then(|f| f.game_app_id)) else {
        bail!("{} isn't in a game; pass --app", args.friend);
    };
    let same_game = friend.and_then(|f| f.game_app_id) == Some(app);
    let friend_lobby = friend.and_then(|f| f.game_lobby_id).filter(|_| same_game);
    let target = if let Some(lobby) = args.lobby {
        JoinTarget::Lobby(lobby)
    } else if let Some(connect) = args.connect.clone().filter(|c| !c.trim().is_empty()) {
        JoinTarget::Connect(connect)
    } else if let Some(lobby) = friend_lobby {
        JoinTarget::Lobby(lobby)
    } else if let Some(connect) = presence_connect.filter(|c| !c.trim().is_empty()) {
        JoinTarget::Connect(connect)
    } else {
        bail!(
            "{} has no joinable lobby or connect string in app {app}; \
             pass --lobby or --connect from their invite",
            args.friend
        );
    };
    Ok((app, target))
}

/// Command-line form of a join.
pub(crate) fn launch_args(target: &JoinTarget) -> Vec<String> {
    match target {
        JoinTarget::Lobby(lobby) => vec!["+connect_lobby".into(), lobby.to_string()],
        JoinTarget::Connect(connect) => connect.split_whitespace().map(String::from).collect(),
    }
}

fn describe(target: &JoinTarget) -> String {
    match target {
        JoinTarget::Lobby(lobby) => format!("lobby {lobby}"),
        JoinTarget::Connect(connect) => format!("\"{connect}\""),
    }
}

/// `aurelia join FRIEND`.
pub(crate) async fn cmd_join(args: JoinArgs, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let friend_id = resolve_user_ident(&client, &args.friend).await?;
    let roster = if daemon::in_daemon() {
        daemon::shared_roster().await
    } else {
        client
            .collect_friends(std::time::Duration::from_secs(3))
            .await?
    };
    let friend = roster.iter().find(|f| f.steam_id == friend_id);
    let needs_presence = args.lobby.is_none()
        && args.connect.is_none()
        && friend.and_then(|f| f.game_lobby_id).is_none();
    let presence = if needs_presence {
        client
            .friend_rich_presence(friend_id)
            .await
            .ok()
            .and_then(|pairs| pairs.into_iter().find(|(k, _)| k == "connect"))
            .map(|(_, v)| v)
    } else {
        None
    };
    let (app, target) = plan(friend, &args, presence)?;

    let told = if daemon::in_daemon() {
        daemon::join_running_game(app, friend_id, &target)
    } else {
        0
    };
    if told > 0 {
        if json {
            print_json(&serde_json::json!({
                "app_id": app, "friend": friend_id, "join": describe(&target), "launched": false,
            }));
        } else {
            cli_println!(
                "Asked running app {app} to join {} ({}).",
                args.friend,
                describe(&target)
            );
        }
        return Ok(());
    }
    if args.no_launch {
        bail!("app {app} isn't running under Aurelia's stand-in; drop --no-launch to start it");
    }
    if !json {
        cli_println!(
            "Starting app {app} to join {} ({}).",
            args.friend,
            describe(&target)
        );
    }
    let play = PlayTarget {
        platform: None,
        steam_mode: None,
        steam_install: None,
        launch_option: None,
        extra_args: launch_args(&target),
    };
    let wait = PlayWait {
        skip_shaders: false,
        launch_timeout: None,
    };
    cmd_play(
        app, None, false, false, false, None, false, false, play, wait, false, json,
    )
    .await
}

#[cfg(test)]
#[path = "join_tests.rs"]
mod tests;
