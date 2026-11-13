use super::*;

#[test]
fn parses_apps_and_playtime_from_localconfig() {
    let vdf = r#"
"UserLocalConfigStore"
{
"Software"
{
    "Valve"
    {
        "Steam"
        {
            "apps"
            {
                "70"
                {
                    "LastPlayed"  "1700762079"
                    "Playtime"    "79"
                }
                "440"
                {
                }
            }
        }
    }
}
}
"#;
    let apps = parse_localconfig_apps(vdf);
    assert_eq!(apps.get(&70), Some(&79));
    assert_eq!(apps.get(&440), Some(&0));
    assert_eq!(apps.len(), 2);
}

#[test]
fn most_recent_id_subtracts_base() {
    // 76561198056839548 - 76561197960265728 = 96573820
    assert_eq!(
        (76_561_198_056_839_548u64).checked_sub(STEAMID64_BASE),
        Some(96_573_820u64)
    );
}

/// Build a minimal v0x29 appinfo.vdf with one app that has a `common`
/// block carrying `name` and `type`, then verify it round-trips.
#[test]
fn parses_minimal_v29_appinfo() {
    // String table: keys referenced by index.
    let keys = ["common", "name", "type", "appid"];
    let idx = |k: &str| keys.iter().position(|&x| x == k).unwrap() as u32;

    let mut kv: Vec<u8> = Vec::new();
    // common { name "Test Game" type "game" }
    kv.push(0x00); // nested object
    kv.extend_from_slice(&idx("common").to_le_bytes());
    kv.push(0x01); // string
    kv.extend_from_slice(&idx("name").to_le_bytes());
    kv.extend_from_slice(b"Test Game\0");
    kv.push(0x01); // string
    kv.extend_from_slice(&idx("type").to_le_bytes());
    kv.extend_from_slice(b"game\0");
    kv.push(0x08); // end common
    kv.push(0x08); // end root

    // Fixed record tail (state, lastUpdated, token, sha1, change, vdfSha1).
    let tail = vec![0u8; 4 + 4 + 8 + 20 + 4 + 20];

    let mut record: Vec<u8> = Vec::new();
    record.extend_from_slice(&tail);
    record.extend_from_slice(&kv);

    let mut file: Vec<u8> = Vec::new();
    file.extend_from_slice(&0x07564429u32.to_le_bytes()); // magic
    file.extend_from_slice(&1u32.to_le_bytes()); // universe
    // Placeholder for string-table offset, patched once we know body length.
    let offset_pos = file.len();
    file.extend_from_slice(&0i64.to_le_bytes());

    // App record: appid, size, record body.
    file.extend_from_slice(&730u32.to_le_bytes());
    file.extend_from_slice(&(record.len() as u32).to_le_bytes());
    file.extend_from_slice(&record);
    // Terminating appid sentinel.
    file.extend_from_slice(&0u32.to_le_bytes());

    // String table.
    let table_offset = file.len() as i64;
    file.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for k in keys {
        file.extend_from_slice(k.as_bytes());
        file.push(0);
    }
    file[offset_pos..offset_pos + 8].copy_from_slice(&table_offset.to_le_bytes());

    let meta = parse_appinfo(&file);
    let app = meta.get(&730).expect("app 730 parsed");
    assert_eq!(app.name, "Test Game");
    assert_eq!(app.app_type, "game");
}

#[test]
fn parses_friend_names_from_localconfig() {
    let vdf = r#"
"UserLocalConfigStore"
{
	"friends"
	{
		"96573820"
		{
			"NameHistory"
			{
				"0"		"OldSelf"
			}
			"avatar"		"ee61dc69"
			"name"		"Self"
		}
		"PersonaName"		"Self"
		"communitypreferences"		"18efc3"
		"160429981"
		{
			"name"		"Jürgen 日本"
			"NameHistory"
			{
				"0"		"Former"
				"name"		"NotThis"
			}
		}
		"78005893"
		{
			"name"		"Say \"hi\" \\o/"
		}
		"25420418"
		{
			"name"		""
		}
		"11111111"
		{
			"avatar"		"abc"
		}
	}
	"Software"
	{
		"friends"
		{
			"22222222"
			{
				"name"		"Nested"
			}
		}
	}
}
"#;
    let names = parse_localconfig_friend_names(vdf);
    assert_eq!(
        names.get(&76561198056839548).map(String::as_str),
        Some("Self")
    );
    assert_eq!(
        names.get(&(STEAMID64_BASE + 160429981)).map(String::as_str),
        Some("Jürgen 日本")
    );
    assert_eq!(
        names.get(&(STEAMID64_BASE + 78005893)).map(String::as_str),
        Some("Say \"hi\" \\o/")
    );
    assert!(!names.contains_key(&(STEAMID64_BASE + 25420418)));
    assert!(!names.contains_key(&(STEAMID64_BASE + 11111111)));
    assert!(!names.contains_key(&(STEAMID64_BASE + 22222222)));
    assert_eq!(names.len(), 3);
}

#[test]
fn friend_names_tolerate_garbage() {
    assert!(parse_localconfig_friend_names("").is_empty());
    assert!(parse_localconfig_friend_names("}}} \"x\" {").is_empty());
    let unterminated = "\"Root\"\n{\n\"friends\"\n{\n\"5\"\n{\n\"name\" \"A";
    assert_eq!(
        parse_localconfig_friend_names(unterminated).get(&(STEAMID64_BASE + 5)),
        Some(&"A".to_string())
    );
}
