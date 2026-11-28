use super::super::*;

const CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"AutoUpdateWindowEnabled\"\t\t\"0\"\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"1245620\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"GE-Proton10-4\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t\t\"ShaderCacheManager\"\n\t\t\t\t{\n\t\t\t\t\t\"HasCurrentBucket\"\t\t\"1\"\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"Music\"\n\t{\n\t}\n}\n";

const ENTRY: &str = "\t\t\t\t\t\"750920\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n";

#[test]
fn adds_entry_and_keeps_the_rest() {
    let out = add_app_mapping(CONFIG, 750920, "proton_experimental")
        .unwrap()
        .unwrap();
    assert!(out.contains(ENTRY), "{out}");
    // Only the entry was inserted.
    assert_eq!(out.replacen(ENTRY, "", 1), CONFIG);
    assert_eq!(
        mappings_from_config(&out),
        vec![
            (750920, "proton_experimental".to_string()),
            (1245620, "GE-Proton10-4".to_string()),
        ]
    );
}

#[test]
fn existing_mapping_is_left_alone() {
    assert_eq!(add_app_mapping(CONFIG, 1245620, "proton_9").unwrap(), None);
}

#[test]
fn empty_name_entry_is_replaced() {
    let raw = CONFIG.replace("GE-Proton10-4", "");
    let out = add_app_mapping(&raw, 1245620, "proton_9").unwrap().unwrap();
    assert_eq!(out.matches("\"1245620\"").count(), 1);
    assert_eq!(
        mappings_from_config(&out),
        vec![(1245620, "proton_9".to_string())]
    );
    assert!(out.contains("\"ShaderCacheManager\""));
}

#[test]
fn missing_mapping_block_is_created() {
    let raw = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"AutoUpdateWindowEnabled\"\t\t\"0\"\n\t\t\t}\n\t\t}\n\t}\n}\n";
    let out = add_app_mapping(raw, 750920, "proton_experimental")
        .unwrap()
        .unwrap();
    let block = format!("\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{{\n{ENTRY}\t\t\t\t}}\n");
    assert_eq!(out.replacen(&block, "", 1), raw);
    assert_eq!(
        mappings_from_config(&out),
        vec![(750920, "proton_experimental".to_string())]
    );
}

#[test]
fn crlf_files_stay_crlf() {
    let raw = CONFIG.replace('\n', "\r\n");
    let out = add_app_mapping(&raw, 750920, "proton_experimental")
        .unwrap()
        .unwrap();
    assert!(!out.replace("\r\n", "").contains('\n'));
    assert_eq!(out.replacen(&ENTRY.replace('\n', "\r\n"), "", 1), raw);
}

#[test]
fn bad_input_is_refused() {
    assert!(add_app_mapping(CONFIG, 1, "bad\"name").is_err());
    assert!(add_app_mapping(CONFIG, 1, "").is_err());
    assert!(add_app_mapping("\"InstallConfigStore\"\n{\n", 1, "proton_9").is_err());
    let no_steam = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t}\n}\n";
    assert!(add_app_mapping(no_steam, 1, "proton_9").is_err());
}

#[test]
fn ensure_backs_up_and_writes_once() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    let path = root.path().join("config").join("config.vdf");
    std::fs::write(&path, CONFIG).unwrap();

    let first = ensure_app_mapping(root.path(), 750920, "proton_experimental").unwrap();
    assert_eq!(first, MappingWrite::Added);
    let backup = root.path().join("config").join("config.vdf.aurelia-backup");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), CONFIG);
    assert!(std::fs::read_to_string(&path).unwrap().contains(ENTRY));

    let second = ensure_app_mapping(root.path(), 750920, "proton_9").unwrap();
    assert_eq!(
        second,
        MappingWrite::Existing("proton_experimental".to_string())
    );
    // No temp files left behind.
    let names: Vec<_> = std::fs::read_dir(root.path().join("config"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn remove_restores_the_original_bytes() {
    let mapped = add_app_mapping(CONFIG, 750920, "proton_experimental")
        .unwrap()
        .unwrap();
    assert_eq!(
        remove_app_mapping(&mapped, 750920).unwrap().unwrap(),
        CONFIG
    );
    // Absent entry or block: nothing.
    assert_eq!(remove_app_mapping(CONFIG, 750920).unwrap(), None);
    let no_block = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t}\n}\n";
    assert_eq!(remove_app_mapping(no_block, 1).unwrap(), None);
    assert!(remove_app_mapping("\"InstallConfigStore\"\n{\n", 1).is_err());
}

#[test]
fn remove_file_backs_up_first() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    let path = root.path().join("config").join("config.vdf");
    std::fs::write(&path, CONFIG).unwrap();

    assert!(remove_app_mapping_file(root.path(), 1245620).unwrap());
    let backup = root.path().join("config").join("config.vdf.aurelia-backup");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), CONFIG);
    assert!(mappings_from_config(&std::fs::read_to_string(&path).unwrap()).is_empty());
    assert!(!remove_app_mapping_file(root.path(), 1245620).unwrap());
}

#[test]
fn windows_tools_are_told_apart() {
    let vdf = "\"compatibilitytools\"\n{\n  \"compat_tools\"\n  {\n    \"GE-Proton11-11-x86_64\"\n    {\n      \"from_oslist\"  \"windows\"\n      \"to_oslist\"    \"linux\"\n    }\n    \"proton-cachyos-native\"\n    {\n      \"from_oslist\"  \"linux\"\n    }\n  }\n}\n";
    let declared = declared_tool_platforms(vdf);
    assert_eq!(
        declared,
        vec![
            ("GE-Proton11-11-x86_64".to_string(), true),
            ("proton-cachyos-native".to_string(), false),
        ]
    );
    assert!(runs_windows_builds("GE-Proton11-11-x86_64", &declared));
    assert!(!runs_windows_builds("proton-cachyos-native", &declared));
    // Undeclared: judged by name.
    assert!(runs_windows_builds("proton_experimental", &declared));
    assert!(runs_windows_builds("GE-Proton-latest", &declared));
    assert!(!runs_windows_builds("steamlinuxruntime_sniper", &declared));
}
