use anyhow::Result;
use xshell::{Shell, cmd};

pub fn run(sh: &Shell) -> Result<()> {
    println!("🔧 Running Universal Fix...");

    // 1. Rust
    println!("🦀 Fixing Rust (fmt & clippy)...");
    cmd!(sh, "cargo fmt").run()?;
    cmd!(
        sh,
        "cargo clippy --workspace --fix --allow-dirty --allow-staged -- -D warnings"
    )
    .run()?;

    // 2. Dashboard
    if sh.path_exists("locald-dashboard") {
        println!("🖥️  Fixing Dashboard (Prettier & ESLint)...");
        let _guard = sh.push_dir("locald-dashboard");
        if !sh.path_exists("node_modules") {
            println!("   Installing dashboard dependencies...");
            cmd!(sh, "pnpm install --silent").run()?;
        }
        cmd!(sh, "npm run format").run()?;
        cmd!(sh, "npx eslint . --fix").run()?;
    }

    println!("✅ All fixes applied.");
    Ok(())
}
