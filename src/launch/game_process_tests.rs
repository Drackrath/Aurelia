use super::game_process::{needs_elevation, windows_command_line};

fn line(args: &[&str]) -> String {
    windows_command_line(args)
}

#[test]
fn plain_args_are_unquoted() {
    assert_eq!(line(&["-dx12", "--fullscreen"]), "-dx12 --fullscreen");
}

#[test]
fn empty_arg_is_quoted() {
    assert_eq!(line(&["a", ""]), r#"a """#);
}

#[test]
fn spaces_are_quoted() {
    assert_eq!(line(&["-path", r"C:\My Games"]), r#"-path "C:\My Games""#);
}

#[test]
fn trailing_backslash_doubled_inside_quotes() {
    assert_eq!(line(&[r"C:\My Games\"]), r#""C:\My Games\\""#);
}

#[test]
fn backslashes_without_quotes_untouched() {
    assert_eq!(line(&[r"C:\Games\x"]), r"C:\Games\x");
}

#[test]
fn embedded_quote_escaped() {
    assert_eq!(line(&[r#"say "hi""#]), r#""say \"hi\"""#);
}

#[test]
fn backslash_before_quote_doubled() {
    assert_eq!(line(&[r#"a\"b"#]), r#""a\\\"b""#);
}

#[test]
fn detects_elevation_error() {
    assert!(needs_elevation(&std::io::Error::from_raw_os_error(740)));
    assert!(!needs_elevation(&std::io::Error::from_raw_os_error(5)));
}
