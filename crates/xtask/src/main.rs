#![allow(clippy::disallowed_methods)]
#![allow(clippy::collapsible_if)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use xshell::{Shell, cmd};

mod docs;
mod assets;
mod e2e;
mod check;
mod verify;
mod util;

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
        Commands::Check { full } => check::run(&sh, full)?,
        Commands::Assets { command } => match command {
            AssetsCommands::Build => assets::build(&sh)?,
        },
        Commands::E2e { command } => match command {
            E2eCommands::Dashboard => e2e::dashboard(&sh)?,
        },
        Commands::Deps { command } => match command {
            DepsCommands::Update => deps_update(&sh)?,
        },
        Commands::Verify { command } => match command {
            VerifyCommands::InstallAssets => verify::install_assets(&sh)?,
            VerifyCommands::Update => verify::update(&sh)?,
            VerifyCommands::Autostart => verify::autostart(&sh)?,
            VerifyCommands::Oci => verify::oci(&sh)?,
            VerifyCommands::Phase33 => verify::phase33(&sh)?,
            VerifyCommands::Phase => verify::phase(&sh)?,
            VerifyCommands::Docs => verify::docs(&sh)?,
            VerifyCommands::Exec => verify::exec(&sh)?,
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
