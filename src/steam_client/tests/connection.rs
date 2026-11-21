#[test]
fn stable_logon_id_is_deterministic_and_nonzero() {
    let a = super::super::connection::stable_logon_id(
        "/home/a/.config/Aurelia|/run/user/1000/aurelia.sock",
    );
    assert_eq!(
        a,
        super::super::connection::stable_logon_id(
            "/home/a/.config/Aurelia|/run/user/1000/aurelia.sock"
        )
    );
    assert_ne!(a, 0);
    assert_ne!(
        a,
        super::super::connection::stable_logon_id(
            "/home/a/.config/heroic/aurelia|/run/user/1000/aurelia.sock"
        )
    );
    assert_ne!(super::super::connection::stable_logon_id(""), 0);
}

#[test]
fn process_logon_id_is_nonzero_and_stable() {
    let first = super::super::connection::logon_id();
    assert_ne!(first, 0);
    assert_eq!(first, super::super::connection::logon_id());
}
