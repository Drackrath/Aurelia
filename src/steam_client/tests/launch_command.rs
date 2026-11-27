use super::super::*;

#[test]
fn command_reads_every_path_as_install_relative() {
    let install = Path::new("/games/install");
    let entry = |executable: &str, workingdir: Option<&str>| LaunchInfo {
        app_id: 1295660,
        id: "0".to_string(),
        description: String::new(),
        executable: executable.to_string(),
        arguments: String::new(),
        workingdir: workingdir.map(str::to_string),
        target: LaunchTarget::WindowsProton,
    };

    for (executable, workingdir, program, cwd) in [
        (r"\Base\Civ7.exe", None, "Base/Civ7.exe", ""),
        (
            "/MGR.app/Contents/MacOS/MGR",
            None,
            "MGR.app/Contents/MacOS/MGR",
            "",
        ),
        (r"\Base\Civ7.exe", Some(r"\Base"), "Base/Civ7.exe", "Base"),
    ] {
        let command = entry(executable, workingdir).command(install);
        assert_eq!(command.program, install.join(program), "{executable}");
        assert_eq!(command.cwd, install.join(cwd), "{workingdir:?}");
    }
}
