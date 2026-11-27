use super::super::table::*;

fn games() -> Table {
    let mut t = Table::new().right("AppId").left("Status").left("Name");
    t.row(["1245620", "installed", "ELDEN RING"]);
    t.row(["10", "-", "Counter-Strike"]);
    t
}

#[test]
fn renders_headers_rule_and_rows() {
    assert_eq!(
        games().render(120),
        "  AppId Status    Name\n  ----- ------    ----\n1245620 installed ELDEN RING\n     10 -         Counter-Strike\n"
    );
}

#[test]
fn no_line_ends_in_spaces() {
    let mut t = Table::new().left("Name").left("Note");
    t.row(["a", ""]);
    for line in t.render(120).lines() {
        assert_eq!(line, line.trim_end(), "trailing space in {line:?}");
    }
}

#[test]
fn last_column_wraps_with_hanging_indent() {
    let mut t = Table::new().left("Flag").left("Help");
    t.row(["--all", "Also list retired DLC the store no longer has"]);
    // Room: 27 - 6 = 21.
    let out = t.render(27);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[2], "--all Also list retired DLC");
    assert_eq!(lines[3], "      the store no longer");
    assert_eq!(lines[4], "      has");
}

#[test]
fn wide_characters_keep_columns_aligned() {
    let mut t = Table::new().left("Name").right("Size");
    t.row(["原神", "1"]);
    t.row(["Doom", "22"]);
    let out = t.render(120);
    let lines: Vec<&str> = out.lines().collect();
    // 原神 is four columns wide.
    assert_eq!(lines[2], "原神    1");
    assert_eq!(lines[3], "Doom   22");
}

#[test]
fn missing_cells_and_newlines_stay_on_one_line() {
    let mut t = Table::new().left("A").left("B").left("C");
    t.row(["x"]);
    t.row(["y", "two\nlines", "z"]);
    let out = t.render(120);
    assert_eq!(out.lines().count(), 4);
    assert!(out.contains("y two lines z"));
}

#[test]
fn wrap_keeps_long_words_whole() {
    assert_eq!(wrap("a verylongword b", 5), vec!["a", "verylongword", "b"]);
    assert_eq!(wrap("", 5), vec![""]);
}
