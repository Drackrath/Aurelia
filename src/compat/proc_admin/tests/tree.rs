use super::super::*;

#[test]
fn tree_includes_spawned_child() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let tree = process_tree_pids(std::process::id() as i32);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(tree.contains(&(child.id() as i32)));
}
