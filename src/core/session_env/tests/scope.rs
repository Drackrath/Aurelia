use crate::core::session_env::*;

#[tokio::test]
async fn scoped_env_replaces_process_env() {
    assert!(!is_scoped());
    let env = SessionEnv::from_pairs([("DISPLAY", ":42")]);
    let seen = scoped(env.clone(), async { (is_scoped(), current()) }).await;
    assert!(seen.0);
    assert_eq!(seen.1, env);
}
