use anyhow::Result;
use xshell::{Shell, cmd};

pub fn install(sh: &Shell) -> Result<()> {
    println!("📦 Building locald (embeds agent + helper)...");
    cmd!(sh, "cargo build --bin locald").run()?;

    println!("🔧 Running admin setup (requires sudo)...");
    cmd!(sh, "sudo target/debug/locald admin setup").run()?;

    println!("✅ Installed. Agent is running in the menu bar.");
    Ok(())
}

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
