//! A snapshot taken, a tree changed every way it can be, and put back.

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("porta-snapshot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_changed_tree_is_described_and_put_back() -> std::io::Result<()> {
    let mount = scratch("mount");
    let copy = scratch("copy");
    std::fs::create_dir_all(mount.join("src/deep"))?;
    std::fs::write(mount.join("kept.txt"), "same")?;
    std::fs::write(mount.join("edited.txt"), "before")?;
    std::fs::write(mount.join("src/deep/gone.txt"), "was here")?;
    std::os::unix::fs::symlink("kept.txt", mount.join("link"))?;
    clone_tree(&mount, &copy)?;

    std::fs::write(mount.join("edited.txt"), "after")?;
    std::fs::remove_dir_all(mount.join("src"))?;
    std::fs::create_dir(mount.join("new-dir"))?;
    std::fs::write(mount.join("new-dir/new.txt"), "new")?;
    std::fs::remove_file(mount.join("link"))?;
    std::os::unix::fs::symlink("edited.txt", mount.join("link"))?;

    let found = changes(&copy, &mount);
    assert_eq!(found.added, vec!["new-dir".to_string(), "new-dir/new.txt".to_string()]);
    assert_eq!(found.changed, vec!["edited.txt".to_string(), "link".to_string()]);
    assert_eq!(found.removed, vec!["src".to_string(), "src/deep".to_string(), "src/deep/gone.txt".to_string()]);

    restore(&copy, &mount, &found)?;
    assert_eq!(changes(&copy, &mount), Changes::default());
    assert_eq!(std::fs::read_to_string(mount.join("src/deep/gone.txt"))?, "was here");
    assert_eq!(std::fs::read_link(mount.join("link"))?, PathBuf::from("kept.txt"));
    std::fs::remove_dir_all(&mount)?;
    std::fs::remove_dir_all(&copy)
}
