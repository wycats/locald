use anyhow::Result;
use xshell::{Shell, cmd};

pub fn dashboard(sh: &Shell) -> Result<()> {
    println!("Running dashboard E2E...");

    // 1. Build Dashboard Assets
    println!("🎨 Building Dashboard assets...");
    cmd!(sh, "pnpm --filter locald-dashboard install").run()?;
    cmd!(sh, "pnpm --filter locald-dashboard build").run()?;

    // 2. Build locald
    println!("🔨 Building locald...");
    cmd!(sh, "cargo build --bin locald").run()?;

    // 3. Prepare E2E environment
    println!("📦 Preparing E2E environment...");
    cmd!(sh, "pnpm --filter locald-dashboard-e2e install").run()?;

    // 4. Install Playwright browsers
    println!("🎭 Installing Playwright browsers...");
    cmd!(
        sh,
        "pnpm --filter locald-dashboard-e2e exec playwright install"
    )
    .run()?;

    // 5. Run tests
    println!("🧪 Running tests...");
    cmd!(sh, "pnpm --filter locald-dashboard-e2e test")
        .env("CI", "true")
        .run()?;

    Ok(())
}
