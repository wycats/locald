#![allow(clippy::disallowed_methods)]
#![allow(clippy::collapsible_if)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use xshell::{Shell, cmd};

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development tasks for locald", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run CI-equivalent checks (fmt, clippy, test)
    Check {
        /// Run in full mode (includes sudo + e2e)
        #[arg(long)]
        full: bool,
    },
    /// Asset management tasks
    Assets {
        #[command(subcommand)]
        command: AssetsCommands,
    },
    /// E2E test tasks
    E2e {
        #[command(subcommand)]
        command: E2eCommands,
    },
    /// Dependency management tasks
    Deps {
        #[command(subcommand)]
        command: DepsCommands,
    },
    /// Verification tasks
    Verify {
        #[command(subcommand)]
        command: VerifyCommands,
    },
    /// Fix code style and lint issues
    Fix,
    /// Run fast build (wraps cargo build with sccache/mold)
    Build {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run fast clippy (wraps cargo clippy with sccache/mold)
    Clippy {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// CI helper tasks
    Ci {
        #[command(subcommand)]
        command: CiCommands,
    },
    /// Sandbox helper tasks
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommands,
    },
    /// Development helper tasks
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },
    /// Run coverage
    Coverage,
    /// Agent workflow tasks
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Prepare phase transition
    Prepare,
    /// Complete phase transition
    Complete { message: String },
    /// Restore context
    Restore,
    /// Resume phase
    Resume,
}

#[derive(Subcommand)]
enum CiCommands {
    /// Watch CI status
    Watch {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, default_value = "CI")]
        workflow: String,
        #[arg(long, default_value = "10")]
        interval: u64,
    },
    /// Check CI logs
    Logs {
        #[arg(long)]
        watch: bool,
    },
    /// Check for untested changes
    Tripwire {
        #[arg(default_value = "origin/main")]
        base: String,
    },
}

#[derive(Subcommand)]
enum SandboxCommands {
    /// Run command in sandbox
    Run {
        sandbox: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum DevCommands {
    /// Run dev server
    Server,
}

#[derive(Subcommand)]
enum AssetsCommands {
    /// Build and sync assets
    Build,
}

#[derive(Subcommand)]
enum E2eCommands {
    /// Run dashboard E2E tests
    Dashboard,
}

#[derive(Subcommand)]
enum DepsCommands {
    /// Update dependencies
    Update,
}

#[derive(Subcommand)]
enum VerifyCommands {
    /// Verify install assets
    InstallAssets,
    /// Verify update
    Update,
    /// Verify autostart
    Autostart,
    /// Verify OCI example
    Oci,
    /// Verify Phase 33 (Draft Mode & History)
    Phase33,
    /// Verify Phase (General)
    Phase,
    /// Verify documentation
    Docs,
    /// Verify Exec Controller
    Exec,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;

    match cli.command {
        Commands::Check { full } => check(&sh, full)?,
        Commands::Assets { command } => match command {
            AssetsCommands::Build => assets_build(&sh)?,
        },
        Commands::E2e { command } => match command {
            E2eCommands::Dashboard => e2e_dashboard(&sh)?,
        },
        Commands::Deps { command } => match command {
            DepsCommands::Update => deps_update(&sh)?,
        },
        Commands::Verify { command } => match command {
            VerifyCommands::InstallAssets => verify_install_assets(&sh)?,
            VerifyCommands::Update => verify_update(&sh)?,
            VerifyCommands::Autostart => verify_autostart(&sh)?,
            VerifyCommands::Oci => verify_oci(&sh)?,
            VerifyCommands::Phase33 => verify_phase33(&sh)?,
            VerifyCommands::Phase => verify_phase(&sh)?,
            VerifyCommands::Docs => verify_docs(&sh)?,
            VerifyCommands::Exec => verify_exec(&sh)?,
        },
        Commands::Fix => fix(&sh)?,
        Commands::Build { args } => fast_build(&sh, args)?,
        Commands::Clippy { args } => fast_clippy(&sh, args)?,
        Commands::Ci { command } => match command {
            CiCommands::Watch {
                run_id,
                branch,
                workflow,
                interval,
            } => ci_watch(&sh, run_id, branch, workflow, interval)?,
            CiCommands::Logs { watch } => ci_logs(&sh, watch)?,
            CiCommands::Tripwire { base } => ci_tripwire(&sh, base)?,
        },
        Commands::Sandbox { command } => match command {
            SandboxCommands::Run { sandbox, args } => sandbox_run(&sh, sandbox, args)?,
        },
        Commands::Dev { command } => match command {
            DevCommands::Server => dev_server(&sh)?,
        },
        Commands::Coverage => coverage(&sh)?,
        Commands::Agent { command } => match command {
            AgentCommands::Prepare => agent_prepare(&sh)?,
            AgentCommands::Complete { message } => agent_complete(&sh, message)?,
            AgentCommands::Restore => agent_restore(&sh)?,
            AgentCommands::Resume => agent_restore(&sh)?,
        },
    }

    Ok(())
}

fn agent_prepare(sh: &Shell) -> Result<()> {
    verify_docs(sh)?;

    println!("=== Active RFCs (To be Completed) ===");
    let rfc_dir = sh.current_dir().join("docs/rfcs");
    let mut active_rfcs = Vec::new();
    if rfc_dir.exists() {
        for entry in std::fs::read_dir(rfc_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let content = std::fs::read_to_string(&path)?;
                if content.lines().any(|l| l.starts_with("stage: 2")) {
                    active_rfcs.push(path);
                }
            }
        }
    }

    if active_rfcs.is_empty() {
        println!("No active RFCs (Stage 2) found. Nothing to transition?");
    } else {
        for rfc in active_rfcs {
            println!("--- {} ---", rfc.file_name().unwrap().to_string_lossy());
            println!("{}", std::fs::read_to_string(rfc)?);
            println!();
        }
    }

    println!("=== Plan Outline ===");
    if sh.path_exists("docs/agent-context/plan-outline.md") {
        println!("--- docs/agent-context/plan-outline.md ---");
        println!(
            "{}",
            std::fs::read_to_string("docs/agent-context/plan-outline.md")?
        );
        println!();
    }

    println!("========================================================");
    println!("REMINDER:");
    println!("1. Update 'docs/agent-context/changelog.md' with completed work.");
    println!("2. Update 'docs/agent-context/decisions.md' with key decisions.");
    println!(
        "3. Update the RFC to Stage 3 (Recommended) and consolidate design into 'docs/agent-context/'."
    );
    println!("4. Run 'cargo xtask agent complete \"<commit_message>\"' to finalize.");
    println!("========================================================");
    Ok(())
}

fn agent_complete(sh: &Shell, message: String) -> Result<()> {
    if message.is_empty() {
        return Err(anyhow::anyhow!("Please provide a commit message."));
    }

    println!("=== Committing Changes ===");
    cmd!(sh, "git add .").run()?;
    cmd!(sh, "git commit -m {message}").run()?;

    println!("=== Checking RFC Status ===");
    let rfc_dir = sh.current_dir().join("docs/rfcs");
    let mut active_rfcs = 0;
    if rfc_dir.exists() {
        for entry in std::fs::read_dir(rfc_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let content = std::fs::read_to_string(&path)?;
                if content.lines().any(|l| l.starts_with("stage: 2")) {
                    active_rfcs += 1;
                }
            }
        }
    }

    if active_rfcs > 0 {
        println!(
            "Warning: There are still {} RFCs in Stage 2 (Available).",
            active_rfcs
        );
        println!("You should update them to Stage 3 (Recommended) if the work is complete.");
    } else {
        println!("No active RFCs found. Transition complete.");
    }

    println!("=== Future Work Context (Stage 0/1 RFCs) ===");
    // Just list files for now
    let _ = cmd!(sh, "ls docs/rfcs").run();

    println!("========================================================");
    println!("NEXT STEPS:");
    println!("1. Review the future work (Stage 0/1 RFCs).");
    println!("2. Select an RFC to work on, or propose a new one (Stage 0).");
    println!("3. Move the selected RFC to Stage 2 (Available) to begin implementation.");
    println!("========================================================");
    Ok(())
}

fn agent_restore(sh: &Shell) -> Result<()> {
    println!("=== Project Goals (Plan Outline) ===");
    if sh.path_exists("docs/agent-context/plan-outline.md") {
        println!(
            "{}",
            std::fs::read_to_string("docs/agent-context/plan-outline.md")?
        );
    } else {
        println!("No plan outline found.");
    }
    println!();

    println!("=== Architecture & Decisions ===");
    if sh.path_exists("docs/agent-context/decisions.md") {
        println!(
            "{}",
            std::fs::read_to_string("docs/agent-context/decisions.md")?
        );
    } else {
        println!("No decisions log found.");
    }
    println!();

    println!("=== Active RFCs (Implementation Context) ===");
    let rfc_dir = sh.current_dir().join("docs/rfcs");
    let mut active_rfcs = Vec::new();
    if rfc_dir.exists() {
        for entry in std::fs::read_dir(rfc_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let content = std::fs::read_to_string(&path)?;
                if content.lines().any(|l| l.starts_with("stage: 2")) {
                    active_rfcs.push(path);
                }
            }
        }
    }

    if active_rfcs.is_empty() {
        println!("No active RFCs (Stage 2).");
    } else {
        for rfc in active_rfcs {
            println!("--- {} ---", rfc.file_name().unwrap().to_string_lossy());
            println!("{}", std::fs::read_to_string(rfc)?);
            println!();
        }
    }
    println!();

    println!("=== Progress (Changelog) ===");
    if sh.path_exists("docs/agent-context/changelog.md") {
        println!(
            "{}",
            std::fs::read_to_string("docs/agent-context/changelog.md")?
        );
    } else {
        println!("No changelog found.");
    }
    println!();

    println!("=== Available Design Docs ===");
    if sh.path_exists("docs/design") {
        let _ = cmd!(sh, "ls docs/design").run();
    }
    Ok(())
}

fn ci_watch(
    sh: &Shell,
    run_id: Option<String>,
    branch: Option<String>,
    workflow: String,
    interval: u64,
) -> Result<()> {
    // This is a simplified version of watch-ci.sh
    let run_id = if let Some(id) = run_id {
        id
    } else {
        let branch = branch.unwrap_or_else(|| {
            cmd!(sh, "git branch --show-current")
                .read()
                .unwrap_or_else(|_| "main".to_string())
        });
        println!(
            "Finding latest run for branch: {} (workflow: {})",
            branch, workflow
        );
        let json = cmd!(
            sh,
            "gh run list --branch {branch} --workflow {workflow} --limit 1 --json databaseId"
        )
        .read()?;
        let json: serde_json::Value = serde_json::from_str(&json)?;
        json[0]["databaseId"]
            .as_i64()
            .ok_or(anyhow::anyhow!("No run found"))?
            .to_string()
    };

    println!("Watching run ID: {}", run_id);
    let interval = interval.to_string();
    cmd!(sh, "gh run watch {run_id} --interval {interval}").run()?;
    Ok(())
}

fn ci_logs(sh: &Shell, watch: bool) -> Result<()> {
    let branch = cmd!(sh, "git branch --show-current").read()?;
    println!("Checking CI status for branch: {}", branch);

    let run_id = cmd!(
        sh,
        "gh run list --branch {branch} --limit 1 --json databaseId --jq '.[0].databaseId'"
    )
    .read()?;
    if run_id.is_empty() {
        return Err(anyhow::anyhow!("No CI runs found for branch {}", branch));
    }
    println!("Latest Run ID: {}", run_id);

    if watch {
        cmd!(sh, "gh run watch {run_id}").run()?;
    }

    cmd!(sh, "gh run view {run_id} --log-failed").run()?;
    Ok(())
}
fn ci_tripwire(sh: &Shell, base: String) -> Result<()> {
    println!("🕵️ Running untested-change tripwire against {}...", base);

    // Ensure base exists
    if cmd!(sh, "git rev-parse --verify {base}")
        .quiet()
        .run()
        .is_err()
    {
        println!("Fetching origin main...");
        cmd!(sh, "git fetch -q origin main").run()?;
    }

    let base_sha = cmd!(sh, "git rev-parse {base}").read()?;
    let head_sha = cmd!(sh, "git rev-parse HEAD").read()?;

    let changed_files = cmd!(sh, "git diff --name-only {base_sha}..{head_sha}").read()?;

    // Check for Rust src changes
    let has_rust_src = changed_files
        .lines()
        .any(|line| (line.starts_with("src/") || line.contains("/src/")) && line.ends_with(".rs"));

    if !has_rust_src {
        println!("✅ No Rust src changes detected.");
        return Ok(());
    }

    // Check for test file changes
    let has_test_files = changed_files.lines().any(|line| {
        line.starts_with("tests/") || line.contains("/tests/") || line.ends_with("_test.rs")
    });

    if has_test_files {
        println!("✅ Test file changes detected.");
        return Ok(());
    }

    // Check for inline tests in diff
    let diff_rs = cmd!(sh, "git diff -U0 {base_sha}..{head_sha} -- *.rs").read()?;

    let has_inline_tests = diff_rs.lines().any(|line| {
        if !line.starts_with('+') && !line.starts_with('-') {
            return false;
        }
        line.contains("#[test]") || line.contains("#[cfg(test)]") || line.contains("mod tests")
    });

    if has_inline_tests {
        println!("✅ Inline test changes detected.");
        return Ok(());
    }

    println!("❌ Rust source changes detected without accompanying tests.");
    println!("   Please add tests or update existing ones.");
    Err(anyhow::anyhow!("Tripwire failed"))
}

fn sandbox_run(sh: &Shell, sandbox: String, args: Vec<String>) -> Result<()> {
    println!("📦 Building locald...");
    cmd!(sh, "cargo build").run()?;

    let locald = sh.current_dir().join("target/debug/locald");
    let log_file = sh.current_dir().join("server.log");

    println!("🚀 Starting locald in sandbox '{}'...", sandbox);
    println!("   Log file: {}", log_file.display());

    // Start server in background
    // xshell doesn't support background processes easily without waiting.
    // We can use std::process::Command
    let log_file_handle = std::fs::File::create(&log_file)?;
    let mut child = std::process::Command::new(&locald)
        .arg("server")
        .arg("start")
        .arg(format!("--sandbox={}", sandbox))
        .stdout(log_file_handle.try_clone()?)
        .stderr(log_file_handle)
        .spawn()?;

    // Wait a bit
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Run client command
    println!("🏃 Running client command...");
    let res = cmd!(sh, "{locald} --sandbox={sandbox}").args(args).run();

    // Cleanup
    println!("🛑 Stopping server...");
    let _ = child.kill();
    let _ = child.wait();

    res?;
    Ok(())
}

fn dev_server(sh: &Shell) -> Result<()> {
    println!("📦 Building locald...");
    cmd!(sh, "cargo build").run()?;

    // Find shim
    // The script used `ls target/debug/build/locald-cli-*/out/shim-target/release/locald-shim | head -n 1`
    // This is fragile. `locald-shim` should be in `target/debug/locald-shim` if we built it with `cargo build -p locald-shim`.
    // But `cargo build` (workspace) might put it there too.
    // Let's assume `target/debug/locald-shim` exists or build it.
    if !sh.path_exists("target/debug/locald-shim") {
        println!("Building locald-shim...");
        cmd!(sh, "cargo build -p locald-shim").run()?;
    }
    let shim_path = "target/debug/locald-shim";

    // Check permissions
    // We can't easily check setuid bit in Rust without `std::os::unix::fs::MetadataExt`.
    // Just try to set it if we can, or warn.
    println!("🔒 Fixing shim permissions (requires sudo)...");
    cmd!(sh, "sudo chown root:root {shim_path}").run()?;
    cmd!(sh, "sudo chmod 4755 {shim_path}").run()?;

    println!("🚀 Starting locald server...");
    cmd!(sh, "target/debug/locald server start").run()?;

    Ok(())
}

fn coverage(sh: &Shell) -> Result<()> {
    if cmd!(sh, "cargo llvm-cov --version").quiet().run().is_err() {
        println!("cargo-llvm-cov is not installed. Installing...");
        cmd!(sh, "cargo install cargo-llvm-cov").run()?;
    }

    println!("Running coverage...");
    cmd!(sh, "cargo llvm-cov --all-features --workspace --html").run()?;

    println!("Coverage report generated at target/llvm-cov/html/index.html");
    if cfg!(target_os = "linux") {
        let _ = cmd!(sh, "xdg-open target/llvm-cov/html/index.html")
            .quiet()
            .run();
    } else if cfg!(target_os = "macos") {
        let _ = cmd!(sh, "open target/llvm-cov/html/index.html")
            .quiet()
            .run();
    }

    Ok(())
}

fn fast_build(sh: &Shell, args: Vec<String>) -> Result<()> {
    setup_fast_env(sh)?;
    let mut cmd = cmd!(sh, "cargo build");
    for arg in args {
        cmd = cmd.arg(arg);
    }
    cmd.run()?;
    Ok(())
}

fn fast_clippy(sh: &Shell, args: Vec<String>) -> Result<()> {
    setup_fast_env(sh)?;
    let mut cmd = cmd!(sh, "cargo clippy");
    for arg in args {
        cmd = cmd.arg(arg);
    }
    cmd.run()?;
    Ok(())
}

fn setup_fast_env(sh: &Shell) -> Result<()> {
    if sh.var("RUSTC_WRAPPER").is_err() && cmd!(sh, "command -v sccache").quiet().run().is_ok() {
        sh.set_var("RUSTC_WRAPPER", "sccache");
    }

    let mut rustflags = sh.var("RUSTFLAGS").unwrap_or_default();
    if cmd!(sh, "command -v mold").quiet().run().is_ok() {
        if !rustflags.contains("-fuse-ld=mold") {
            rustflags.push_str(" -C link-arg=-fuse-ld=mold");
        }
    } else if cmd!(sh, "command -v lld").quiet().run().is_ok() {
        if !rustflags.contains("-fuse-ld=lld") {
            rustflags.push_str(" -C link-arg=-fuse-ld=lld");
        }
    }
    sh.set_var("RUSTFLAGS", rustflags);
    Ok(())
}

fn fix(sh: &Shell) -> Result<()> {
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

fn check(sh: &Shell, full: bool) -> Result<()> {
    println!("Running checks (full={})...", full);

    let sandbox = "check";

    println!("Running rustfmt...");
    cmd!(sh, "cargo fmt --all -- --check").run()?;

    println!("Running cargo build...");
    cmd!(sh, "cargo build").run()?;

    println!("Checking CLI surface manifest is up to date...");
    let locald = "./target/debug/locald";
    let mut output = cmd!(sh, "{locald} __surface cli-manifest").read()?;
    if !output.ends_with('\n') {
        output.push('\n');
    }
    sh.write_file("/tmp/locald-cli-manifest.json", &output)?;
    cmd!(
        sh,
        "diff -u docs/surface/cli-manifest.json /tmp/locald-cli-manifest.json"
    )
    .run()?;

    println!("Running clippy...");
    cmd!(sh, "cargo clippy --workspace -- -D warnings").run()?;

    println!("Building dashboard (CI-equivalent)...");
    cmd!(
        sh,
        "pnpm --filter locald-dashboard install --frozen-lockfile"
    )
    .env("CI", "1")
    .run()?;
    cmd!(sh, "pnpm --filter locald-dashboard build").run()?;

    println!("Running docs checks...");
    verify_docs(sh)?;

    println!("Running docs build...");
    cmd!(sh, "pnpm --filter locald-docs install --frozen-lockfile")
        .env("CI", "1")
        .run()?;
    cmd!(sh, "pnpm --filter locald-docs build").run()?;

    println!("Running IPC verification...");
    let _env_guard_http = sh.push_env("LOCALD_HTTP_PORT", "8080");
    let _env_guard_https = sh.push_env("LOCALD_HTTPS_PORT", "8443");

    // Start server in background
    let mut child = std::process::Command::new(locald)
        .arg("server")
        .arg("start")
        .arg(format!("--sandbox={}", sandbox))
        .env("LOCALD_HTTP_PORT", "8080")
        .env("LOCALD_HTTPS_PORT", "8443")
        .spawn()?;

    // Wait a bit
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Ping
    let ping_result = cmd!(sh, "{locald} ping --sandbox={sandbox}").read();

    // Shutdown
    let _ = cmd!(sh, "{locald} server shutdown --sandbox={sandbox}").run();
    let _ = child.wait();

    match ping_result {
        Ok(output) if output.contains("Pong") => println!("IPC Ping successful"),
        _ => {
            println!("IPC Ping failed");
            return Err(anyhow::anyhow!("IPC Ping failed"));
        }
    }

    println!("Running unit tests...");
    cmd!(
        sh,
        "cargo test --tests --workspace --all-features --exclude locald-e2e"
    )
    .run()?;

    if full {
        println!("Running full checks (sudo + e2e)...");
        // Ensure binaries exist
        cmd!(sh, "cargo build -p locald-cli --all-features").run()?;
        cmd!(sh, "cargo build -p locald-shim").run()?;

        // Install shim
        println!("Installing privileged shim (requires sudo)...");
        cmd!(sh, "sudo {locald} --sandbox=prepush admin setup").run()?;

        // Re-install shim (just in case)
        cmd!(sh, "sudo {locald} --sandbox=prepush admin setup").run()?;

        // Run e2e
        cmd!(sh, "cargo test -p locald-e2e --tests --all-features").run()?;
    }

    println!("All checks passed.");
    Ok(())
}

fn assets_build(sh: &Shell) -> Result<()> {
    println!("Building assets...");

    let server_assets = "crates/locald-server/src/assets";
    let _ = sh.remove_path(server_assets);
    sh.create_dir(server_assets)?;

    // Dashboard
    println!("Building Dashboard...");
    cmd!(sh, "pnpm --filter locald-dashboard install").run()?;
    cmd!(sh, "pnpm --filter locald-dashboard build").run()?;

    // Copy dashboard assets
    // Check build/ or .svelte-kit/output/client/
    if sh.path_exists("locald-dashboard/build") {
        println!("Copying Dashboard build...");
        cmd!(sh, "cp -r locald-dashboard/build/. {server_assets}/").run()?;
    } else if sh.path_exists("locald-dashboard/.svelte-kit/output/client") {
        println!("Copying Dashboard client output...");
        cmd!(
            sh,
            "cp -r locald-dashboard/.svelte-kit/output/client/. {server_assets}/"
        )
        .run()?;
    } else {
        return Err(anyhow::anyhow!("Could not find dashboard build output"));
    }

    // Docs
    println!("Building Docs...");
    cmd!(sh, "pnpm --filter locald-docs install").run()?;
    cmd!(sh, "pnpm --filter locald-docs build").run()?;

    if sh.path_exists("locald-docs/dist") {
        println!("Copying Docs build...");
        sh.create_dir(format!("{}/docs", server_assets))?;
        cmd!(sh, "cp -r locald-docs/dist/. {server_assets}/docs/").run()?;
    } else {
        return Err(anyhow::anyhow!("Could not find docs build output"));
    }

    println!("Assets updated successfully in {}", server_assets);
    Ok(())
}

fn e2e_dashboard(sh: &Shell) -> Result<()> {
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

fn deps_update(sh: &Shell) -> Result<()> {
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

fn verify_install_assets(sh: &Shell) -> Result<()> {
    println!("Verifying install assets...");

    // Check requirements
    let reqs = ["git", "cargo", "pnpm", "curl"];
    for req in reqs {
        if cmd!(sh, "command -v {req}").quiet().run().is_err() {
            return Err(anyhow::anyhow!("missing required command: {}", req));
        }
    }

    let repo_root = cmd!(sh, "git rev-parse --show-toplevel").read()?;
    let sandbox_name = format!("verify-assets-{}", std::process::id());
    let tmp_dir = sh.create_temp_dir()?;
    let tmp_path = tmp_dir.path();
    let wt_dir = tmp_path.join("worktree");
    let install_root = tmp_path.join("install-root");
    let log_file = tmp_path.join("locald-server.log");

    // Cleanup logic is handled by TempDir drop, but we need to remove worktree and shutdown locald
    // We can't easily do "trap cleanup EXIT" in Rust.
    // We'll use a helper struct or just ensure we clean up at the end.
    // For the daemon, we'll try to shutdown.

    println!("==> Creating temporary worktree");
    cmd!(sh, "git -C {repo_root} worktree add --detach {wt_dir} HEAD").run()?;

    // Sync local working tree
    let status = cmd!(sh, "git -C {repo_root} status --porcelain=v1").read()?;
    if !status.is_empty() {
        println!("==> Syncing local working tree into worktree");
        // Use tar to sync
        // xshell doesn't support pipes easily. We can use std::process::Command or just run the shell command.
        // But we want to avoid bash.
        // We can use `tar` command directly if we construct it carefully.
        // Or just use `rsync` if available? `tar` is more standard.
        // Let's use `sh -c` for the pipe part since it's a one-off.
        let exclude_flags = "--exclude=.git --exclude=**/node_modules --exclude=**/target --exclude=**/dist --exclude=**/build --exclude=**/.svelte-kit --exclude=**/.turbo --exclude=**/.next";
        let cmd_str = format!(
            "tar -C {repo_root} {exclude_flags} -cf - . | tar -C {wt_dir:?} -xf -",
            repo_root = repo_root,
            wt_dir = wt_dir
        );
        // We can't easily run this with xshell cmd! macro because of the pipe.
        // We'll use std::process::Command with "sh -c".
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd_str)
            .status()?;
        if !status.success() {
            return Err(anyhow::anyhow!("Failed to sync worktree"));
        }
    }

    // Remove prebuilt assets in worktree
    let _ = sh.remove_path(wt_dir.join("locald-dashboard/build"));
    let _ = sh.remove_path(wt_dir.join("locald-docs/dist"));

    println!("==> Installing locald into temporary prefix");
    let locald_cli_path = wt_dir.join("crates/locald-cli");
    cmd!(
        sh,
        "cargo install --path {locald_cli_path} --locked --root {install_root} --force"
    )
    .run()?;

    let locald_bin = install_root.join("bin/locald");

    println!("==> Starting locald daemon (sandbox: {})", sandbox_name);

    // Start in background
    let mut child = std::process::Command::new(&locald_bin)
        .arg("--sandbox")
        .arg(&sandbox_name)
        .arg("server")
        .arg("start")
        .env("LOCALD_HTTP_PORT", "0")
        .env("LOCALD_HTTPS_PORT", "0")
        .stdout(std::fs::File::create(&log_file)?)
        .stderr(std::fs::File::create(&log_file)?) // Capture stderr too
        .spawn()?;

    // Wait for port
    let mut port = String::new();
    for _ in 0..120 {
        if let Ok(content) = sh.read_file(&log_file) {
            if let Some(line) = content
                .lines()
                .find(|l| l.contains("Proxy bound to http://"))
            {
                // Extract port
                // "Proxy bound to http://0.0.0.0:12345"
                if let Some(p) = line.split(':').next_back() {
                    port = p.trim().to_string();
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    if port.is_empty() {
        println!("error: failed to detect HTTP proxy port from logs");
        println!("---- locald log tail ----");
        let _ = cmd!(sh, "tail -n 200 {log_file}").run();
        let _ = child.kill();
        return Err(anyhow::anyhow!("Failed to detect port"));
    }

    println!("==> Detected HTTP port: {}", port);

    let check_host = |host: &str, path: &str| -> Result<()> {
        let url = format!("http://127.0.0.1:{}{}", port, path);
        let status = std::process::Command::new("curl")
            .arg("-sS")
            .arg("-o")
            .arg("/dev/null")
            .arg("-w")
            .arg("%{http_code}")
            .arg("-H")
            .arg(format!("Host: {}", host))
            .arg(&url)
            .output()?;

        let code = String::from_utf8_lossy(&status.stdout);
        if code.trim() != "200" {
            println!(
                "error: expected 200 for Host={} {} (got {})",
                host, path, code
            );
            println!("---- locald log tail ----");
            let _ = cmd!(sh, "tail -n 200 {log_file}").run();
            return Err(anyhow::anyhow!("Check failed"));
        }
        Ok(())
    };

    println!("==> Checking embedded dashboard");
    check_host("locald.localhost", "/")?;

    println!("==> Checking embedded docs");
    check_host("docs.localhost", "/")?;

    println!("==> Shutting down daemon");
    let _ = cmd!(sh, "{locald_bin} --sandbox {sandbox_name} server shutdown").run();
    let _ = child.wait();

    // Cleanup worktree
    let _ = cmd!(sh, "git -C {repo_root} worktree remove --force {wt_dir}").run();

    println!("OK: installed locald serves embedded dashboard + docs");
    Ok(())
}

fn verify_update(sh: &Shell) -> Result<()> {
    println!("Verifying update...");
    let sandbox = "update-test";

    println!("Building locald (v1)...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald_path = sh.current_dir().join("target/debug/locald");
    let locald = locald_path.to_str().unwrap();

    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} --sandbox={sandbox} server shutdown").run();
    // pkill is risky, let's rely on shutdown.

    println!("Starting locald server...");
    let mut child = std::process::Command::new(locald)
        .arg("--sandbox")
        .arg(sandbox)
        .arg("server")
        .arg("start")
        .spawn()?;

    println!("Waiting for server...");
    let mut up = false;
    for _ in 0..50 {
        if cmd!(sh, "{locald} --sandbox={sandbox} ping")
            .quiet()
            .run()
            .is_ok()
        {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if !up {
        let _ = child.kill();
        return Err(anyhow::anyhow!("Server failed to start"));
    }

    // Get PID
    // We can't easily get the "real" PID if it's a wrapper, but locald server start should be the process.
    // But wait, `locald server start` might fork? No, it usually runs in foreground if not daemonized.
    // The script used `pgrep`.
    // We can use `locald ping` to get info? No, ping just returns Pong.
    // We can check the PID file if we knew where it was.
    // Or we can trust `child.id()` if it doesn't fork.
    // The script used `pgrep -f "locald.*server start"`.
    // Let's try to use `pgrep` via xshell if available, or just assume `child.id()` is close enough if we can't.
    // But `locald` might be a wrapper.
    // Let's use `pgrep` to match the script logic.
    let real_pid = cmd!(sh, "pgrep -f 'locald.*server start'").read()?;
    let real_pid = real_pid
        .lines()
        .last()
        .ok_or(anyhow::anyhow!("Failed to find server PID"))?
        .trim()
        .to_string();
    println!("Initial Server PID: {}", real_pid);

    println!("Touching crates/locald-core/src/lib.rs to force version bump...");
    // We need to actually change the mtime.
    // xshell doesn't have touch.
    // Use `touch` command.
    cmd!(sh, "touch crates/locald-core/src/lib.rs").run()?;
    std::thread::sleep(std::time::Duration::from_secs(1));

    println!("Building locald (v2)...");
    cmd!(sh, "cargo build -p locald-cli").run()?;

    println!("Running 'locald up'...");
    // Use a valid project path
    let example_dir = "examples/adhoc-test";
    let _guard = sh.push_dir(example_dir);
    cmd!(sh, "{locald} --sandbox={sandbox} up").run()?;
    drop(_guard);

    // Check new PID
    let new_pid = cmd!(sh, "pgrep -f 'locald.*server start'").read()?;
    let new_pid = new_pid
        .lines()
        .last()
        .ok_or(anyhow::anyhow!("Failed to find new server PID"))?
        .trim()
        .to_string();
    println!("New Server PID: {}", new_pid);

    if real_pid == new_pid {
        let _ = child.kill();
        return Err(anyhow::anyhow!(
            "Error: Server PID did not change. Auto-restart failed."
        ));
    } else {
        println!(
            "Success: Server PID changed ({} -> {}). Auto-restart worked.",
            real_pid, new_pid
        );
    }

    println!("Cleaning up...");
    let _ = cmd!(sh, "{locald} --sandbox={sandbox} server shutdown").run();
    let _ = child.wait();

    Ok(())
}

fn verify_autostart(sh: &Shell) -> Result<()> {
    println!("Verifying autostart...");

    // Build locald
    println!("Building locald...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald = sh.current_dir().join("target/debug/locald");

    // Stop existing daemon
    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} stop").quiet().run();
    let _ = cmd!(sh, "pkill -f 'locald server'").quiet().run();
    std::thread::sleep(std::time::Duration::from_secs(1));

    if cmd!(sh, "pgrep -f 'locald server'").quiet().run().is_ok() {
        return Err(anyhow::anyhow!("Error: Daemon failed to stop."));
    }
    println!("Daemon is stopped.");

    // Run 'locald try'
    println!("Running 'locald try' (should auto-start daemon)...");
    let example_dir = "examples/adhoc-test";
    let _guard = sh.push_dir(example_dir);

    // Disable privileged ports for testing
    let _env_guard = sh.push_env("LOCALD_PRIVILEGED_PORTS", "false");

    cmd!(sh, "{locald} try echo 'Hello from try'").run()?;

    // Verify daemon is running
    if cmd!(sh, "pgrep -f 'locald server'").quiet().run().is_err() {
        return Err(anyhow::anyhow!("Error: Daemon was NOT auto-started."));
    }
    println!("Success: Daemon was auto-started.");

    // Register project explicitly since try skipped it (non-interactive)
    println!("Registering project (locald up)...");
    cmd!(sh, "{locald} up").run()?;

    // Run 'locald run'
    println!("Running 'locald run'...");
    cmd!(sh, "{locald} run web echo 'Hello from run'").run()?;

    Ok(())
}

fn verify_oci(sh: &Shell) -> Result<()> {
    println!("Verifying OCI example...");

    // Check AppArmor
    if let Ok(val) = sh.read_file("/proc/sys/kernel/apparmor_restrict_unprivileged_userns") {
        if val.trim() == "1" {
            println!("WARNING: AppArmor is restricting unprivileged user namespaces.");
            // We can't easily fix this from here without sudo, but we can warn.
            // The script tried `unshare -U`.
            if cmd!(sh, "unshare -U echo 'User NS check passed'")
                .quiet()
                .run()
                .is_err()
            {
                return Err(anyhow::anyhow!(
                    "ERROR: Unable to create user namespace. Please check AppArmor settings."
                ));
            }
        }
    }

    println!("Building oci-example (and locald)...");
    cmd!(sh, "cargo build -p oci-example -p locald-cli --bin locald").run()?;

    // Check shim setup
    // We can't easily check setuid bit with xshell directly without ls -l parsing or stat.
    // But we can just try to run it or warn.
    // The script checked LOCALD_SETUP_SHIM env var.
    if std::env::var("LOCALD_SETUP_SHIM").unwrap_or_default() == "1" {
        println!("Installing/repairing privileged locald-shim (requires sudo)...");
        cmd!(sh, "sudo target/debug/locald admin setup").run()?;
    } else {
        println!("Note: The example requires a privileged locald-shim (setuid root).");
        println!("If needed, run: sudo target/debug/locald admin setup");
    }

    println!("Running oci-example...");
    cmd!(
        sh,
        "cargo run -p oci-example -- alpine:latest echo 'Hello from inside the container!'"
    )
    .run()?;

    println!("Success!");
    Ok(())
}

fn verify_phase33(sh: &Shell) -> Result<()> {
    println!("Verifying Phase 33 (Draft Mode & History)...");

    // Build locald
    println!("Building locald...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald = sh.current_dir().join("target/debug/locald");

    // Stop daemon
    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} stop").quiet().run();
    let _ = cmd!(sh, "pkill -f 'locald server'").quiet().run();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Disable privileged ports
    let _env_guard = sh.push_env("LOCALD_PRIVILEGED_PORTS", "false");

    // Clean up
    let _ = sh.remove_path("examples/adhoc-test/locald.toml");
    let home = std::env::var("HOME").unwrap();
    let history_path = format!("{}/.local/share/locald/history", home);
    let _ = sh.remove_path(&history_path);

    // 1. Test 'locald try'
    println!("Testing 'locald try' & Auto-start...");
    let example_dir = "examples/adhoc-test";
    let _guard = sh.push_dir(example_dir);
    cmd!(sh, "{locald} try echo 'Draft Mode Test'").run()?;

    // Verify daemon
    if cmd!(sh, "pgrep -f 'locald server'").quiet().run().is_err() {
        return Err(anyhow::anyhow!("Error: Daemon failed to auto-start."));
    }

    // 2. Test History
    println!("Testing History...");
    let history = sh.read_file(&history_path)?;
    let last_cmd = history.lines().last().unwrap_or("");
    if last_cmd != "echo Draft Mode Test" {
        return Err(anyhow::anyhow!(
            "Error: History mismatch. Expected 'echo Draft Mode Test', got '{}'",
            last_cmd
        ));
    }

    // 3. Test 'locald add last'
    println!("Testing 'locald add last'...");
    cmd!(sh, "{locald} add last --name 'draft-service'").run()?;

    Ok(())
}

fn verify_docs(sh: &Shell) -> Result<()> {
    println!("Checking documentation...");

    // Check active RFCs
    let rfc_dir = sh.current_dir().join("docs/rfcs");
    let mut active_rfcs = 0;
    if rfc_dir.exists() {
        for entry in std::fs::read_dir(rfc_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let content = std::fs::read_to_string(&path)?;
                if content.lines().any(|l| l.starts_with("stage: 2")) {
                    active_rfcs += 1;
                }
            }
        }
    }

    if active_rfcs == 0 {
        println!("Note: No active RFCs (Stage 2: Available) found.");
        println!("If you are implementing a feature, ensure you have an RFC in Stage 2.");
    } else {
        println!("Found {} active RFC(s).", active_rfcs);
    }

    // Check plan-outline.md
    if !sh.path_exists("docs/agent-context/plan-outline.md") {
        println!("Warning: docs/agent-context/plan-outline.md not found.");
    }

    verify_docs_screenshots(sh)?;
    verify_docs_sidebar(sh)?;
    verify_docs_cli(sh)?;

    println!("Documentation checks passed.");
    Ok(())
}

fn verify_phase(sh: &Shell) -> Result<()> {
    println!("Verifying Phase...");
    check(sh, false)?;
    verify_docs(sh)?;

    println!("Running Phase 99 checks...");
    cmd!(sh, "cargo test -p locald-utils").run()?;
    cmd!(sh, "cargo test -p locald-e2e --test cgroup_cleanup").run()?;

    Ok(())
}

fn verify_exec(sh: &Shell) -> Result<()> {
    println!("Verifying Exec Controller...");
    cmd!(sh, "cargo build -p locald-cli").run()?;

    // Kill existing
    let _ = cmd!(sh, "pkill -f locald").quiet().run();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let server_bin = "./target/debug/locald";
    let server_log = "/tmp/locald-server.log";
    let server_log_file = std::fs::File::create(server_log)?;

    println!("Starting locald-server...");
    let mut child = std::process::Command::new(server_bin)
        .arg("server")
        .arg("start")
        .env("RUST_LOG", "info")
        .stdout(server_log_file.try_clone()?)
        .stderr(server_log_file)
        .spawn()?;

    std::thread::sleep(std::time::Duration::from_secs(2));

    println!("Adding exec service...");
    // Note: passing command as string with quotes for sh -c
    let cmd_str = "while true; do echo 'Hello from ExecController'; sleep 1; done";

    let res = (|| -> Result<()> {
        cmd!(
            sh,
            "{server_bin} service add exec --name test-exec -- {cmd_str}"
        )
        .run()?;

        std::thread::sleep(std::time::Duration::from_secs(5));

        println!("Listing services...");
        cmd!(sh, "{server_bin} status").run()?;

        println!("Checking service logs...");
        let logs = cmd!(sh, "{server_bin} logs test-exec").read()?;
        println!("{}", logs);

        if logs.contains("Hello from ExecController") {
            println!("SUCCESS: Logs received from ExecController");
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "FAILURE: Logs NOT received from ExecController"
            ))
        }
    })();

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();
    let _ = cmd!(sh, "pkill -f locald").quiet().run();

    res
}

// --- Documentation Verification ---

#[derive(serde::Deserialize)]
struct Manifest {
    root: CommandNode,
}

#[derive(serde::Deserialize, Clone)]
struct CommandNode {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    args: Vec<ArgNode>,
    #[serde(default)]
    subcommands: Vec<CommandNode>,
}

#[derive(serde::Deserialize, Clone)]
struct ArgNode {
    long: Option<String>,
    short: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    positional: bool,
}

fn verify_docs_screenshots(sh: &Shell) -> Result<()> {
    println!("Checking screenshots...");
    let docs_root = sh.current_dir().join("locald-docs/src/content/docs");
    let screenshots_root = sh.current_dir().join("locald-docs/public/screenshots");

    if !docs_root.exists() || !screenshots_root.exists() {
        println!("Skipping screenshot check (dirs not found)");
        return Ok(());
    }

    let mut referenced = std::collections::HashSet::new();
    let re = regex::Regex::new(r"/(screenshots/[^\s)\]\x22']+\.png)")?;

    for entry in walkdir::WalkDir::new(&docs_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "md" || ext == "mdx" {
                let content = std::fs::read_to_string(path)?;
                for cap in re.captures_iter(&content) {
                    referenced.insert(cap[1].to_string());
                }
            }
        }
    }

    let mut missing = Vec::new();
    for rel in &referenced {
        let file_name = rel.strip_prefix("screenshots/").unwrap_or(rel);
        let path = screenshots_root.join(file_name);
        if !path.exists() {
            missing.push(rel.clone());
        }
    }

    if !missing.is_empty() {
        println!("Missing screenshots:");
        for m in missing {
            println!("  - {}", m);
        }
        return Err(anyhow::anyhow!("Missing screenshots found"));
    }

    // Check for unused
    let mut unused = Vec::new();
    for entry in walkdir::WalkDir::new(&screenshots_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy();
        let rel = format!("screenshots/{}", file_name);
        if !referenced.contains(&rel) {
            unused.push(rel);
        }
    }

    if !unused.is_empty() {
        println!("Unused screenshots (warning):");
        for u in unused {
            println!("  - {}", u);
        }
    }

    Ok(())
}

fn verify_docs_sidebar(sh: &Shell) -> Result<()> {
    println!("Checking sidebar links...");
    let config_path = sh.current_dir().join("locald-docs/astro.config.mjs");
    if !config_path.exists() {
        println!("Skipping sidebar check (config not found)");
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let re = regex::Regex::new(r"\blink\s*:\s*['\x22]([^'\x22]+)['\x22]")?;

    let mut seen = std::collections::HashMap::new();
    let mut dups = std::collections::HashMap::new();

    for cap in re.captures_iter(&content) {
        let link = &cap[1];
        let normalized = if link == "/" {
            "/".to_string()
        } else {
            let l = if !link.starts_with('/') {
                format!("/{}", link)
            } else {
                link.to_string()
            };
            if l.ends_with('/') {
                l
            } else {
                format!("{}/", l)
            }
        };

        if let Some(count) = seen.get_mut(&normalized) {
            *count += 1;
            dups.insert(normalized.clone(), *count);
        } else {
            seen.insert(normalized, 1);
        }
    }

    if !dups.is_empty() {
        println!("Duplicate sidebar links detected:");
        for (link, count) in dups {
            println!("  - {} (occurrences: {})", link, count);
        }
        return Err(anyhow::anyhow!("Duplicate sidebar links found"));
    }

    Ok(())
}

fn verify_docs_cli(sh: &Shell) -> Result<()> {
    println!("Checking CLI surface docs...");
    let manifest_path = sh.current_dir().join("docs/surface/cli-manifest.json");
    if !manifest_path.exists() {
        println!("Skipping CLI check (manifest not found)");
        return Ok(());
    }

    let manifest: Manifest = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let root = manifest.root;

    // Build global index
    let mut global_long = std::collections::HashSet::new();
    let mut global_short = std::collections::HashSet::new();
    for arg in &root.args {
        if arg.global && !arg.positional {
            if let Some(l) = &arg.long {
                global_long.insert(l.clone());
            }
            if let Some(s) = &arg.short {
                global_short.insert(s.clone());
            }
            for a in &arg.aliases {
                global_long.insert(a.clone());
            }
        }
    }

    let docs_root = sh.current_dir().join("locald-docs/src/content/docs");
    let readme = sh.current_dir().join("README.md");

    let mut files = Vec::new();
    if readme.exists() {
        files.push(readme);
    }
    if docs_root.exists() {
        for entry in walkdir::WalkDir::new(&docs_root) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "md" || ext == "mdx" {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
    }

    let mut errors = Vec::new();

    for file_path in files {
        let content = std::fs::read_to_string(&file_path)?;
        let invocations = extract_locald_invocations(&content);

        for inv in invocations {
            if let Err(e) = validate_invocation(&root, &global_long, &global_short, &inv.tokens) {
                errors.push(format!(
                    "{}:{}: {} (Snippet: {})",
                    file_path.display(),
                    inv.line,
                    e,
                    inv.tokens.join(" ")
                ));
            }
        }
    }

    if !errors.is_empty() {
        println!("CLI surface docs lint failed:");
        for e in errors {
            println!("- {}", e);
        }
        return Err(anyhow::anyhow!("CLI surface docs lint failed"));
    }

    Ok(())
}

struct Invocation {
    line: usize,
    tokens: Vec<String>,
}

fn extract_locald_invocations(content: &str) -> Vec<Invocation> {
    let mut invocations = Vec::new();
    let mut in_fence = false;
    let mut fence_lang = String::new();

    for (i, line) in content.lines().enumerate() {
        let line_trim = line.trim();
        if line_trim.starts_with("```") {
            if !in_fence {
                in_fence = true;
                fence_lang = line_trim.trim_start_matches("```").trim().to_lowercase();
            } else {
                in_fence = false;
                fence_lang.clear();
            }
            continue;
        }

        if !in_fence {
            continue;
        }

        if !fence_lang.is_empty()
            && !["bash", "sh", "shell", "zsh", "console", "terminal", ""]
                .contains(&fence_lang.as_str())
        {
            continue;
        }

        let cleaned = strip_comment(&strip_prompt(line)).trim().to_string();
        if cleaned.is_empty() {
            continue;
        }

        let mut tokens = shlex(&cleaned);
        if tokens.is_empty() {
            continue;
        }

        // Skip env vars
        while !tokens.is_empty() && tokens[0].contains('=') && !tokens[0].starts_with('-') {
            if regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=")
                .unwrap()
                .is_match(&tokens[0])
            {
                tokens.remove(0);
            } else {
                break;
            }
        }

        if tokens.is_empty() {
            continue;
        }
        if tokens[0] == "sudo" {
            tokens.remove(0);
        }
        if tokens.is_empty() {
            continue;
        }

        if tokens[0] != "locald" {
            continue;
        }

        invocations.push(Invocation {
            line: i + 1,
            tokens,
        });
    }
    invocations
}

fn validate_invocation(
    root: &CommandNode,
    global_long: &std::collections::HashSet<String>,
    global_short: &std::collections::HashSet<String>,
    tokens: &[String],
) -> Result<()> {
    let mut tokens = tokens.to_vec();
    tokens.remove(0); // consume locald

    let mut current = root;

    // Greedy descent
    while !tokens.is_empty() {
        let next = &tokens[0];
        if next.starts_with('-') {
            break;
        } // Flag

        // Find subcommand
        let mut found = None;
        for sub in &current.subcommands {
            if sub.name == *next || sub.aliases.contains(next) {
                found = Some(sub);
                break;
            }
        }

        if let Some(sub) = found {
            current = sub;
            tokens.remove(0);
        } else {
            break; // Positional or unknown
        }
    }

    // Validate flags
    for token in tokens {
        if token == "--" {
            break;
        }
        if !token.starts_with('-') {
            break;
        } // Stop at first non-flag (positional or value)

        if token.starts_with("--") {
            let name = token.trim_start_matches("--").split('=').next().unwrap();
            if global_long.contains(name) {
                continue;
            }

            let mut found = false;
            for arg in &current.args {
                if let Some(l) = &arg.long {
                    if l == name {
                        found = true;
                        break;
                    }
                }
                for a in &arg.aliases {
                    if a == name {
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err(anyhow::anyhow!("Unknown flag: --{}", name));
            }
        } else {
            // Short flags
            let cluster = token.trim_start_matches('-');
            for ch in cluster.chars() {
                let s = ch.to_string();
                if global_short.contains(&s) {
                    continue;
                }

                let mut found = false;
                for arg in &current.args {
                    if let Some(sh) = &arg.short {
                        if sh == &s {
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    return Err(anyhow::anyhow!("Unknown flag: -{}", ch));
                }
            }
        }
    }

    Ok(())
}

fn shlex(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' && !in_single {
            if let Some(next) = chars.next() {
                cur.push(next);
            }
            continue;
        }
        if !in_double && ch == '\'' {
            in_single = !in_single;
            continue;
        }
        if !in_single && ch == '"' {
            in_double = !in_double;
            continue;
        }
        if !in_single && !in_double && ch.is_whitespace() {
            if !cur.is_empty() {
                tokens.push(cur);
                cur = String::new();
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn strip_prompt(line: &str) -> String {
    let line = line.trim_start();
    if let Some(stripped) = line.strip_prefix("$ ") {
        stripped.to_string()
    } else {
        line.to_string()
    }
}

fn strip_comment(line: &str) -> String {
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = line.char_indices();

    while let Some((i, ch)) = chars.next() {
        if ch == '\\' {
            chars.next();
            continue;
        }
        if !in_double && ch == '\'' {
            in_single = !in_single;
        } else if !in_single && ch == '"' {
            in_double = !in_double;
        } else if !in_single && !in_double && ch == '#' {
            return line[..i].trim_end().to_string();
        }
    }
    line.to_string()
}
