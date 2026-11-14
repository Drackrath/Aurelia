use super::*;

fn friend(app: Option<u32>, lobby: Option<u64>) -> Friend {
    Friend {
        steam_id: 9,
        relationship: 3,
        persona_name: Some("Alice".into()),
        persona_state: Some(1),
        game_app_id: app,
        game_name: None,
        game_lobby_id: lobby,
        avatar_hash: None,
    }
}

fn args() -> JoinArgs {
    JoinArgs {
        friend: "Alice".into(),
        connect: None,
        lobby: None,
        app: None,
        no_launch: false,
    }
}

#[test]
fn friend_lobby_beats_presence() {
    let f = friend(Some(480), Some(5));
    let plan = plan(Some(&f), &args(), Some("+connect x".into())).unwrap();
    assert_eq!(plan, (480, JoinTarget::Lobby(5)));
}

#[test]
fn presence_connect_without_lobby() {
    let f = friend(Some(480), None);
    let plan = plan(Some(&f), &args(), Some("+connect 1.2.3.4".into())).unwrap();
    assert_eq!(plan, (480, JoinTarget::Connect("+connect 1.2.3.4".into())));
}

#[test]
fn explicit_options_win() {
    let f = friend(Some(480), Some(5));
    let mut a = args();
    a.connect = Some("+connect inv".into());
    assert_eq!(
        plan(Some(&f), &a, None).unwrap().1,
        JoinTarget::Connect("+connect inv".into())
    );
    a.connect = None;
    a.lobby = Some(77);
    assert_eq!(plan(Some(&f), &a, None).unwrap().1, JoinTarget::Lobby(77));
}

#[test]
fn other_app_ignores_friend_lobby() {
    let f = friend(Some(480), Some(5));
    let mut a = args();
    a.app = Some(70);
    assert!(plan(Some(&f), &a, None).is_err());
    a.connect = Some("+connect y".into());
    assert_eq!(plan(Some(&f), &a, None).unwrap().0, 70);
}

#[test]
fn nothing_joinable_fails() {
    let idle = friend(None, None);
    let err = plan(Some(&idle), &args(), None).unwrap_err().to_string();
    assert!(err.contains("isn't in a game"), "{err}");
    let playing = friend(Some(480), None);
    assert!(plan(Some(&playing), &args(), Some("  ".into())).is_err());
    assert!(plan(None, &args(), None).is_err());
}

#[test]
fn launch_args_per_target() {
    assert_eq!(launch_args(&JoinTarget::Lobby(5)), ["+connect_lobby", "5"]);
    assert_eq!(
        launch_args(&JoinTarget::Connect(
            "+connect 1.2.3.4:27015 +password x".into()
        )),
        ["+connect", "1.2.3.4:27015", "+password", "x"]
    );
}

#[test]
fn extra_args_append_to_options() {
    use crate::commands::launch::append_args;
    let extra = launch_args(&JoinTarget::Lobby(5));
    assert_eq!(
        append_args("-novid  -console", &extra),
        "-novid -console +connect_lobby 5"
    );
    assert_eq!(append_args("", &extra), "+connect_lobby 5");
}
