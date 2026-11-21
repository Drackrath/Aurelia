use super::super::*;

const CONFIG: &str = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"0"
					{
						"name"		"GE-Proton10-4"
						"config"		""
						"priority"		"75"
					}
					"1245620"
					{
						"name"		"proton_experimental"
						"config"		""
						"priority"		"250"
					}
					"2784470"
					{
						"name"		"GE-Proton11-12-x86_64"
						"config"		""
						"priority"		"250"
					}
					"440"
					{
						"name"		""
						"config"		""
						"priority"		"250"
					}
				}
			}
		}
	}
}
"#;

const TOOL: &str = r#""compatibilitytools"
{
  "compat_tools"
  {
    "GE-Proton11-12-x86_64" // Internal name of this tool
    {
      "install_path" "."
      "display_name" "GE-Proton11-12"
      "from_oslist"  "windows"
      "to_oslist"    "linux"
    }
  }
}
"#;

#[test]
fn reads_tool_names_and_mappings() {
    assert_eq!(declared_tool_names(TOOL), vec!["GE-Proton11-12-x86_64"]);
    let mappings = mappings_from_config(CONFIG);
    assert_eq!(
        mappings,
        vec![
            (0, "GE-Proton10-4".to_string()),
            (1245620, "proton_experimental".to_string()),
            (2784470, "GE-Proton11-12-x86_64".to_string()),
        ]
    );
}

#[test]
fn flags_only_missing_custom_tools() {
    let declared = declared_tool_names(TOOL);
    let classified = classify(mappings_from_config(CONFIG), &declared);
    let dangling: Vec<_> = classified.iter().filter(|m| !m.known).collect();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].app_id, 0);
    assert_eq!(dangling[0].tool, "GE-Proton10-4");
}

#[test]
fn valve_tools_are_always_known() {
    for name in [
        "proton_9",
        "proton_experimental",
        "SteamLinuxRuntime_sniper",
        "legacysteamruntime",
    ] {
        assert!(is_valve_tool(name), "{name}");
    }
    assert!(!is_valve_tool("GE-Proton10-4"));
}

#[test]
fn suggestion_skips_latest_folders() {
    let tool = |name: &str, folder: &str| (name.to_string(), folder.to_string());
    let tools = vec![
        tool("GE-Proton11-12-x86_64", "GE-Proton-latest"),
        tool("GE-Proton11-11", "GE-Proton11-11"),
        tool("GE-Proton9-20", "GE-Proton9-20"),
    ];
    assert_eq!(suggest_tool(&tools), "GE-Proton11-11");
    assert_eq!(suggest_tool(&[]), "proton_experimental");
}
