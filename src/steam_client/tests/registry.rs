use super::super::*;

#[test]
fn reg_dword_parses_hex_and_decimal() {
    // `reg query` prints REG_DWORD data as hex, e.g. SteamPID / ActiveUser.
    assert_eq!(parse_reg_dword("0x2a3c"), Some(0x2a3c));
    assert_eq!(parse_reg_dword("  0x0 "), Some(0));
    assert_eq!(parse_reg_dword("1234"), Some(1234));
    assert_eq!(parse_reg_dword("0xzz"), None);
    assert_eq!(parse_reg_dword(""), None);
}
