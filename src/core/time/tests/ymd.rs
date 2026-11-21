use super::super::*;

#[test]
fn unix_to_ymd_known_dates() {
    assert_eq!(unix_to_ymd(0), "1970-01-01");
    assert_eq!(unix_to_ymd(1_700_000_000), "2023-11-14"); // 2023-11-14T22:13:20Z
    assert_eq!(unix_to_ymd(1_009_843_200), "2002-01-01"); // exact midnight UTC
    // Leap day round-trips correctly.
    assert_eq!(unix_to_ymd(1_582_934_400), "2020-02-29");
}
