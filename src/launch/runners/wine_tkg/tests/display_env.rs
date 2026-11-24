use std::collections::HashMap;

use crate::core::session_env::{SessionEnv, scoped};
use crate::launch::runners::wine_tkg::env::insert_display_env;

#[tokio::test]
async fn forwarded_request_uses_client_display() {
    // Client on Wayfire, daemon elsewhere.
    let client = SessionEnv::from_pairs([("DISPLAY", ":6"), ("WAYLAND_DISPLAY", "wayland-4")]);
    let env = scoped(client, async {
        let mut env = HashMap::new();
        insert_display_env(&mut env);
        env
    })
    .await;
    assert_eq!(env.get("DISPLAY").map(String::as_str), Some(":6"));
    assert_eq!(
        env.get("WAYLAND_DISPLAY").map(String::as_str),
        Some("wayland-4")
    );
    assert!(!env.contains_key("XAUTHORITY"));
}
