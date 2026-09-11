//! `social` command handlers.

use crate::daemon;
use crate::output;

use crate::commands::common::*;

use anyhow::Result;

/// `aurelia friends`: list the logged-in user's friends with status and game.
///
/// Inside the daemon the roster is served from the background watcher's cache;
/// standalone it does a short best-effort collection over a fresh connection
/// (which may be partial — see `SteamClient::collect_friends`).
pub(crate) async fn cmd_friends(json: bool) -> Result<()> {
    let friends = if daemon::in_daemon() {
        daemon::shared_roster().await
    } else {
        let client = authed_client().await?;
        let mut all = client
            .collect_friends(std::time::Duration::from_secs(3))
            .await?;
        // Match the daemon view: only actual friends (relationship 3).
        all.retain(|f| f.relationship == 3);
        all
    };

    if json {
        print_json(&friends);
        return Ok(());
    }

    if friends.is_empty() {
        cli_println!(
            "No friends to show yet. (If you just started a session, the roster fills a moment \
             after connecting — try again.)"
        );
        return Ok(());
    }

    cli_println!("{:<20}  {:<17}  {:<12}  GAME", "STATUS", "STEAMID", "NAME");
    for f in &friends {
        let name = f.persona_name.as_deref().unwrap_or("?");
        let game = match (&f.game_name, f.game_app_id) {
            (Some(g), _) => g.clone(),
            (None, Some(id)) => format!("app {id}"),
            (None, None) => String::new(),
        };
        cli_println!(
            "{:<20}  {:<17}  {:<12}  {}",
            persona_state_label(f.persona_state),
            f.steam_id,
            name,
            game
        );
    }
    cli_println!("\n{} friend(s).", friends.len());
    Ok(())
}

/// `aurelia user IDENT`: profile over the CM.
pub(crate) async fn cmd_user(user: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let steam_id = resolve_user_ident(&client, &user).await?;
    let profile = client.user_profile(steam_id).await?;
    if json {
        print_json(&profile);
        return Ok(());
    }
    cli_println!(
        "{}  ({})",
        profile.persona_name.as_deref().unwrap_or("(no persona name)"),
        profile.steam_id
    );
    cli_println!("Profile  : {}", profile.profile_url);
    if let Some(url) = &profile.avatar_url {
        cli_println!("Avatar   : {url}");
    }
    let visibility = match profile.visibility {
        3 => "public",
        2 => "friends-only",
        1 => "private",
        _ => "unknown",
    };
    let limited = if profile.is_limited { ", limited account" } else { "" };
    cli_println!("Privacy  : {visibility}{limited}");
    if let Some(name) = &profile.real_name {
        cli_println!("Name     : {name}");
    }
    if let Some(place) = &profile.location {
        cli_println!("Location : {place}");
    }
    if let Some(t) = profile.created {
        cli_println!("Created  : {}", aurelia::steam_client::unix_to_ymd(t as i64));
    }
    cli_println!("Status   : {}", persona_state_label(profile.persona_state));
    match (&profile.game_name, profile.game_app_id) {
        (Some(g), _) => cli_println!("Playing  : {g}"),
        (None, Some(id)) => cli_println!("Playing  : app {id}"),
        _ => {}
    }
    if let Some(t) = profile.last_logoff {
        cli_println!("Last seen: {}", aurelia::steam_client::unix_to_ymd(t as i64));
    }
    if let Some(h) = &profile.headline {
        cli_println!("Headline : {h}");
    }
    if let Some(s) = &profile.summary {
        cli_println!("\n{}", aurelia::web::store::strip_html(s));
    }
    if let Some(t) = profile.ban_expires {
        cli_println!("\nCommunity ban until {}", aurelia::steam_client::unix_to_ymd(t as i64));
    }
    Ok(())
}

/// `aurelia friends search IDENT`: same as `user`.
pub(crate) async fn cmd_friends_search(query: String, json: bool) -> Result<()> {
    cmd_user(query, json).await
}

/// `aurelia friends add IDENT`: send a friend request.
pub(crate) async fn cmd_friends_add(query: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let steam_id = resolve_user_ident(&client, &query).await?;
    let added = client.add_friend(steam_id).await?;
    let name = added.persona_name;

    if json {
        print_json(&serde_json::json!({
            "steam_id": added.steam_id,
            "persona_name": name,
            "status": "request_sent",
        }));
    } else {
        match &name {
            Some(n) => cli_println!("Friend request sent to {n} ({}).", added.steam_id),
            None => cli_println!("Friend request sent to {}.", added.steam_id),
        }
    }
    Ok(())
}

/// `aurelia friends remove <steamid>`: remove a friend or cancel a pending request.
pub(crate) async fn cmd_friends_remove(steamid: u64, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.remove_friend(steamid).await?;
    if json {
        print_json(&serde_json::json!({ "steam_id": steamid, "status": "removed" }));
    } else {
        cli_println!("Removed {steamid} from your friends.");
    }
    Ok(())
}

/// `aurelia chat send <steamid> <message>`: send a direct message to a friend.
pub(crate) async fn cmd_chat_send(steamid: u64, message: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let sent = client.send_chat_message(steamid, message).await?;
    if json {
        print_json(&serde_json::json!({
            "steamid": steamid,
            "sent": true,
            "server_timestamp": sent.server_timestamp,
            "modified_message": sent.modified_message,
        }));
    } else {
        cli_println!("Message sent to {steamid}.");
    }
    Ok(())
}

/// `aurelia chat history <steamid>`: show recent messages with a friend.
pub(crate) async fn cmd_chat_history(steamid: u64, count: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let messages = client.recent_chat_messages(steamid, count).await?;
    if json {
        print_json(&messages);
        return Ok(());
    }
    if messages.is_empty() {
        cli_println!("No messages with {steamid}.");
        return Ok(());
    }
    for m in &messages {
        let who = if m.from_self { "me" } else { "them" };
        cli_println!("[{}] {:>4}: {}", m.timestamp, who, m.message);
    }
    Ok(())
}

/// Render one incoming chat event for `aurelia chat open`, filtered to the
/// conversation with `target`. Plain mode prints a friendly line; `--json` emits
/// one NDJSON event for GUI drivers.
pub(crate) fn render_chat_event(target: u64, event: steam_vent_chat::ChatEvent, json: bool) {
    use steam_vent_chat::ChatEvent;
    // Every event carries the conversation partner's SteamID; ignore events for
    // other conversations sharing this connection's notification stream.
    let source = match &event {
        ChatEvent::Message(e) | ChatEvent::EchoMessage(e) => u64::from(e.source),
        ChatEvent::Typing(e) => u64::from(e.source),
    };
    if source != target {
        return;
    }
    match event {
        ChatEvent::Message(e) => {
            let text = e.message_no_bbcode.unwrap_or(e.message);
            if json {
                print_json_line(&serde_json::json!({
                    "event": "message", "from": target, "text": text,
                    "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("them: {text}");
            }
        }
        ChatEvent::EchoMessage(e) => {
            let text = e.message_no_bbcode.unwrap_or(e.message);
            if json {
                print_json_line(&serde_json::json!({
                    "event": "echo", "to": target, "text": text,
                    "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("me (sent elsewhere): {text}");
            }
        }
        ChatEvent::Typing(e) => {
            if json {
                print_json_line(&serde_json::json!({
                    "event": "typing", "from": target, "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("* {target} is typing...");
            }
        }
    }
}

/// `aurelia chat open <steamid>`: interactive live chat. Incoming messages stream
/// to stdout while lines read from stdin are sent to the friend; the session ends
/// on stdin EOF (Ctrl-D / Ctrl-Z) or when the notification stream closes.
///
/// Runs naturally over the daemon — the thin client streams stdin and relays
/// stdout in real time — so it reuses the shared, already-online connection.
pub(crate) async fn cmd_chat_open(steamid: u64, json: bool) -> Result<()> {
    use tokio_stream::StreamExt;

    let client = authed_client().await?;
    // Ensure the session is announced online so Steam delivers incoming messages
    // (a no-op effect if the daemon's watcher already did this).
    if let Err(e) = client.announce_configured_presence().await {
        tracing::warn!("could not announce presence: {e:#}");
    }

    let chat = client.chat_client()?;
    let mut events = chat.listen();

    if json {
        print_json_line(&serde_json::json!({ "event": "ready", "with": steamid }));
    } else {
        cli_println!(
            "Chat with {steamid}. Type a message and press Enter to send; Ctrl-D (Ctrl-Z on \
             Windows) to quit."
        );
    }

    loop {
        tokio::select! {
            incoming = events.next() => {
                match incoming {
                    Some(Ok(event)) => render_chat_event(steamid, event, json),
                    Some(Err(_)) => continue, // dropped/lagged notification — keep going
                    None => break,            // connection closed
                }
            }
            line = output::read_line_opt() => {
                match line {
                    Ok(Some(text)) => {
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        if let Err(e) = client.send_chat_message(steamid, text.to_string()).await {
                            if json {
                                print_json_line(&serde_json::json!({
                                    "event": "error", "message": format!("{e:#}"),
                                }));
                            } else {
                                cli_eprintln!("send failed: {e:#}");
                            }
                        }
                    }
                    Ok(None) => break, // stdin closed
                    Err(_) => break,
                }
            }
        }
    }

    if json {
        print_json_line(&serde_json::json!({ "event": "closed", "with": steamid }));
    }
    Ok(())
}
