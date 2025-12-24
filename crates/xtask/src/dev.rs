use anyhow::Result;
use xshell::{Shell, cmd};

pub fn server(sh: &Shell) -> Result<()> {
    println!("📦 Building locald...");
    cmd!(sh, "cargo build").run()?;

    if !sh.path_exists("target/debug/locald-shim") {
        println!("Building locald-shim...");
        cmd!(sh, "cargo build -p locald-shim").run()?;
    }
    let shim_path = "target/debug/locald-shim";

    println!("🔒 Fixing shim permissions (requires sudo)...");
    cmd!(sh, "sudo chown root:root {shim_path}").run()?;
    cmd!(sh, "sudo chmod 4755 {shim_path}").run()?;

    println!("🚀 Starting locald server...");
    cmd!(sh, "target/debug/locald server start").run()?;

    Ok(())
}
