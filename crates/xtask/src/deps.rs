use anyhow::Result;
use xshell::{cmd, Shell};

pub fn update(sh: &Shell) -> Result<()> {
    println!("Updating deps...");

    println!("Updating Cargo.lock to latest compatible versions...");
    cmd!(sh, "cargo update").run()?;
    println!("Cargo.lock updated.");

    println!("Checking for major version updates...");
    if cmd!(sh, "command -v cargo-outdated").quiet().run().is_ok() {
        cmd!(sh, "cargo outdated --workspace --root-deps-only").run()?;
    } else {
        println!("cargo-outdated not found. Skipping major version check.");
        println!("To install: cargo install cargo-outdated");
    }

    Ok(())
}
