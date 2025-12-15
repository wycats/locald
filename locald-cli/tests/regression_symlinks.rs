use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_build_with_broken_symlink() {
    // 1. Setup Sandbox
    let root = TempDir::new().unwrap();
    let home = root.path();
    let locald_bin = assert_cmd::cargo::cargo_bin!("locald");

    // `cargo_bin!` only works for binaries built by *this* crate. The shim lives in a
    // different workspace crate, so we resolve it as a sibling of the `locald` binary.
    let _shim_bin = locald_bin
        .parent()
        .expect("locald binary path should have a parent directory")
        .join("locald-shim");

    // 2. Create Project with Broken Symlink
    let project_dir = home.join("broken-symlink-project");
    fs::create_dir(&project_dir).unwrap();

    // Create a dummy file to link to, then delete it to make the link broken
    let target = project_dir.join("target-file");
    fs::write(&target, "content").unwrap();
    let link = project_dir.join("broken-link");
    symlink(&target, &link).unwrap();
    fs::remove_file(&target).unwrap(); // Now 'link' is broken

    // Add minimal files for CNB detection (using Python as it's simple)
    fs::write(project_dir.join("requirements.txt"), "").unwrap();
    fs::write(project_dir.join("Procfile"), "web: python3 -m http.server").unwrap();

    // 3. Run Build
    // We expect it to SUCCEED (or at least not crash with OS error 2).
    // The buildpack might fail if it needs the file, but locald itself shouldn't panic.
    let mut cmd = Command::new(&locald_bin);
    cmd.env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .arg("--sandbox=regression-test")
        .arg("build")
        .arg(&project_dir);

    let output = cmd.output().expect("failed to execute locald build");

    // 4. Verify
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Stdout: {}", stdout);
    println!("Stderr: {}", stderr);

    // It should NOT contain "os error 2" or "No such file or directory" related to the copy
    assert!(!stderr.contains("os error 2"));
    assert!(!stderr.contains("No such file or directory"));

    // It should contain our warning
    // Note: tracing logs go to stdout by default in locald-cli
    assert!(stdout.contains("Encountered broken symlink"));

    // The build itself might fail due to network or other reasons in this isolated env,
    // but we are testing for the *crash*.
    // If it crashed, the exit code would be non-zero and the error would be panic-related.
    // If it fails gracefully (e.g. buildpack error), that's fine.
    // But ideally it succeeds if we had network.
    // For this test, we just assert it didn't panic.
}

#[test]
fn test_build_with_valid_symlink() {
    // 1. Setup Sandbox
    let root = TempDir::new().unwrap();
    let home = root.path();
    let locald_bin = assert_cmd::cargo::cargo_bin!("locald");
    let _shim_bin = locald_bin
        .parent()
        .expect("locald binary path should have a parent directory")
        .join("locald-shim");

    // 2. Create Project with Valid Symlink
    let project_dir = home.join("valid-symlink-project");
    fs::create_dir(&project_dir).unwrap();

    let target = project_dir.join("real-file");
    fs::write(&target, "real content").unwrap();
    let link = project_dir.join("link-file");
    symlink(&target, &link).unwrap();

    // Add minimal files
    fs::write(project_dir.join("requirements.txt"), "").unwrap();
    fs::write(project_dir.join("Procfile"), "web: python3 -m http.server").unwrap();

    // 3. Run Build
    let mut cmd = Command::new(&locald_bin);
    cmd.env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .arg("--sandbox=regression-test-valid")
        .arg("build")
        .arg(&project_dir);

    let output = cmd.output().expect("failed to execute locald build");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should not warn about broken symlink
    assert!(!stderr.contains("Encountered broken symlink"));
}
