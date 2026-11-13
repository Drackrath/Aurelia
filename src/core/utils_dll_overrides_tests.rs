use super::{dll_override_entries, dll_override_mode, dll_override_names, merge_dll_overrides};

fn e(name: &str, mode: &str) -> (String, String) {
    (name.to_string(), mode.to_string())
}

#[test]
fn replaces_dll_inside_comma_group() {
    let merged = merge_dll_overrides("d3d11,dxgi=n", &[e("d3d11", "b")]);
    assert_eq!(merged, "dxgi=n;d3d11=b");
}

#[test]
fn matches_dll_suffix_and_case() {
    let merged = merge_dll_overrides("D3D11.dll=n;dxgi=n", &[e("d3d11", "b")]);
    assert_eq!(merged, "dxgi=n;d3d11=b");
    let merged = merge_dll_overrides("d3d11=n", &[e("D3D11.DLL", "b")]);
    assert_eq!(merged, "D3D11.DLL=b");
}

#[test]
fn empty_input_has_no_leading_separator() {
    assert_eq!(merge_dll_overrides("", &[e("d3d11", "n")]), "d3d11=n");
    assert_eq!(merge_dll_overrides(";;", &[e("dxgi", "b")]), "dxgi=b");
    assert_eq!(merge_dll_overrides("", &[]), "");
}

#[test]
fn policy_entry_replaced_not_duplicated() {
    let policy = "steamclient=n;d3d11=b;dxgi=b";
    let merged = merge_dll_overrides(policy, &[e("d3d11", "n")]);
    assert_eq!(merged, "steamclient=n;dxgi=b;d3d11=n");
    assert_eq!(merged.matches("d3d11").count(), 1);
}

#[test]
fn keeps_empty_mode_entries() {
    let merged = merge_dll_overrides("lsteamclient=;dxgi=n", &[e("dxgi", "b")]);
    assert_eq!(merged, "lsteamclient=;dxgi=b");
}

#[test]
fn last_entry_wins_for_same_dll() {
    let merged = merge_dll_overrides("", &[e("dxgi", "n"), e("DXGI", "b")]);
    assert_eq!(merged, "DXGI=b");
}

#[test]
fn mode_and_names_lookup() {
    let s = "d3d11,dxgi=n,b;Xlive.dll=b";
    assert_eq!(dll_override_mode(s, "dxgi").as_deref(), Some("n,b"));
    assert_eq!(dll_override_mode(s, "xlive").as_deref(), Some("b"));
    assert_eq!(dll_override_mode(s, "d3d9"), None);
    let names = dll_override_names(s);
    assert!(names.contains("d3d11") && names.contains("dxgi") && names.contains("xlive"));
}

#[test]
fn fill_keeps_existing_and_adds_missing() {
    let merged = super::fill_dll_overrides(
        "d3d11,dxgi=n;steamclient=n",
        &[e("DXGI.dll", "b"), e("xlive", "b")],
    );
    assert_eq!(merged, "d3d11,dxgi=n;steamclient=n;xlive=b");
    assert_eq!(super::fill_dll_overrides("", &[e("a", "n")]), "a=n");
}

#[test]
fn entries_expand_comma_groups() {
    assert_eq!(
        dll_override_entries("a,b=n;c=b"),
        vec![e("a", "n"), e("b", "n"), e("c", "b")]
    );
}
