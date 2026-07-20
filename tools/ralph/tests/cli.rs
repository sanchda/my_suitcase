use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch() -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "ralph-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(path.join(".ralph")).unwrap();
    path
}

#[test]
fn schema_is_embedded_and_bypasses_project_config() {
    let root = scratch();
    fs::write(root.join(".ralph/ralph.toml"), "this is invalid toml = [").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .arg("schema")
        .current_dir(&root)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, include_bytes!("../BACKLOG.schema.md"));
    assert!(output.stderr.is_empty());
}

#[test]
fn help_lists_schema_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("ralph schema"));
}
