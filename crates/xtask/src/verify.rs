use anyhow::Result;
use sysinfo::Pid;
use xshell::{Shell, cmd};

use crate::{check, docs, util};

pub fn install_assets(sh: &Shell) -> Result<()> {
    println!("Verifying install assets...");

    // Check requirements
    let _ = util::which::require("git")?;
    let _ = util::which::require("cargo")?;
    let _ = util::which::require("pnpm")?;

    let repo_root = std::path::PathBuf::from(cmd!(sh, "git rev-parse --show-toplevel").read()?);
    let sandbox_name = format!("verify-assets-{}", std::process::id());
    let tmp_dir = sh.create_temp_dir()?;
    let tmp_path = tmp_dir.path();
    let wt_dir = tmp_path.join("worktree");
    let install_root = tmp_path.join("install-root");
    let log_file = tmp_path.join("locald-server.log");

    println!("==> Creating temporary worktree");
    cmd!(sh, "git -C {repo_root} worktree add --detach {wt_dir} HEAD").run()?;

    // Sync local working tree
    let status = cmd!(sh, "git -C {repo_root} status --porcelain=v1").read()?;
    if !status.is_empty() {
        println!("==> Syncing local working tree into worktree");
        util::fs::sync_tree(&repo_root, &wt_dir)?;
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
        let tail = util::fs::read_tail_bytes(&log_file, 64 * 1024).unwrap_or_default();
        println!("{}", tail);
        let _ = child.kill();
        return Err(anyhow::anyhow!("Failed to detect port"));
    }

    println!("==> Detected HTTP port: {}", port);

    let check_host = |host: &str, path: &str| -> Result<()> {
        let url = format!("http://127.0.0.1:{}{}", port, path);
        let _ = url; // keep for error messages

        let code = util::http::get_status_code("127.0.0.1", &port, host, path)?;
        if code != 200 {
            println!(
                "error: expected 200 for Host={} {} (got {})",
                host, path, code
            );
            println!("---- locald log tail ----");
            let tail = util::fs::read_tail_bytes(&log_file, 64 * 1024).unwrap_or_default();
            println!("{}", tail);
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

pub fn update(sh: &Shell) -> Result<()> {
    println!("Verifying update...");
    let sandbox = "update-test";

    println!("Building locald (v1)...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald_path = sh.current_dir().join("target/debug/locald");
    let locald = locald_path.to_str().unwrap();

    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} --sandbox={sandbox} server shutdown").run();

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

    let find_pid = || -> Result<Pid> {
        let sandbox_flag = format!("--sandbox={}", sandbox);
        let pids = util::process::find_pids_matching(|p| {
            util::process::cmd_any_contains(p, &sandbox_flag)
                && util::process::cmd_any_eq(p, "server")
                && util::process::cmd_any_eq(p, "start")
        });
        pids.into_iter()
            .last()
            .ok_or_else(|| anyhow::anyhow!("Failed to find server PID"))
    };

    let real_pid = find_pid()?;
    println!("Initial Server PID: {:?}", real_pid);

    println!("Bumping crates/locald-core/src/lib.rs mtime to force rebuild...");
    util::fs::bump_mtime("crates/locald-core/src/lib.rs")?;
    std::thread::sleep(std::time::Duration::from_secs(1));

    println!("Building locald (v2)...");
    cmd!(sh, "cargo build -p locald-cli").run()?;

    println!("Running 'locald up'...");
    let example_dir = "examples/adhoc-test";
    let _guard = sh.push_dir(example_dir);
    cmd!(sh, "{locald} --sandbox={sandbox} up").run()?;
    drop(_guard);

    // Check new PID
    let new_pid = find_pid()?;
    println!("New Server PID: {:?}", new_pid);

    if real_pid == new_pid {
        let _ = child.kill();
        return Err(anyhow::anyhow!(
            "Error: Server PID did not change. Auto-restart failed."
        ));
    } else {
        println!(
            "Success: Server PID changed ({real_pid:?} -> {new_pid:?}). Auto-restart worked."
        );
    }

    println!("Cleaning up...");
    let _ = cmd!(sh, "{locald} --sandbox={sandbox} server shutdown").run();
    let _ = child.wait();

    Ok(())
}

pub fn autostart(sh: &Shell) -> Result<()> {
    println!("Verifying autostart...");

    // Build locald
    println!("Building locald...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald = sh.current_dir().join("target/debug/locald");

    // Stop existing daemon
    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} stop").quiet().run();
    let pids = util::process::find_pids_matching(|p| {
        util::process::cmd_any_contains(p, "locald") && util::process::cmd_any_eq(p, "server")
    });
    let _ = util::process::kill_pids(&pids, util::process::KillStrategy::TermThenKill);
    std::thread::sleep(std::time::Duration::from_secs(1));

    let still = util::process::find_pids_matching(|p| {
        util::process::cmd_any_contains(p, "locald") && util::process::cmd_any_eq(p, "server")
    });
    if !still.is_empty() {
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
    let running = util::process::find_pids_matching(|p| {
        util::process::cmd_any_contains(p, "locald") && util::process::cmd_any_eq(p, "server")
    });
    if running.is_empty() {
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

pub fn oci(sh: &Shell) -> Result<()> {
    println!("Verifying OCI example...");

    // Check AppArmor
    if let Ok(val) = sh.read_file("/proc/sys/kernel/apparmor_restrict_unprivileged_userns") {
        if val.trim() == "1" {
            println!("WARNING: AppArmor is restricting unprivileged user namespaces.");
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

pub fn phase33(sh: &Shell) -> Result<()> {
    println!("Verifying Phase 33 (Draft Mode & History)...");

    // Build locald
    println!("Building locald...");
    cmd!(sh, "cargo build -p locald-cli").run()?;
    let locald = sh.current_dir().join("target/debug/locald");

    // Stop daemon
    println!("Stopping any existing daemon...");
    let _ = cmd!(sh, "{locald} stop").quiet().run();
    let pids = util::process::find_pids_matching(|p| {
        util::process::cmd_any_contains(p, "locald") && util::process::cmd_any_eq(p, "server")
    });
    let _ = util::process::kill_pids(&pids, util::process::KillStrategy::TermThenKill);
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
    let running = util::process::find_pids_matching(|p| {
        util::process::cmd_any_contains(p, "locald") && util::process::cmd_any_eq(p, "server")
    });
    if running.is_empty() {
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

pub fn docs(sh: &Shell) -> Result<()> {
    docs::verify_docs(sh)
}

pub fn phase(sh: &Shell) -> Result<()> {
    println!("Verifying Phase...");
    check::run(sh, false)?;

    println!("Running Phase 99 checks...");
    cmd!(sh, "cargo test -p locald-utils").run()?;
    cmd!(sh, "cargo test -p locald-e2e --test cgroup_cleanup").run()?;

    Ok(())
}

pub fn exec(sh: &Shell) -> Result<()> {
    println!("Verifying Exec Controller...");
    cmd!(sh, "cargo build -p locald-cli").run()?;

    // Kill existing
    let pids = util::process::find_pids_matching(|p| util::process::cmd_any_contains(p, "locald"));
    let _ = util::process::kill_pids(&pids, util::process::KillStrategy::TermThenKill);
    std::thread::sleep(std::time::Duration::from_secs(1));

    let server_bin = "./target/debug/locald";
    let server_log = "/tmp/locald-server.log";
    let server_log_file = std::fs::File::create(server_log)?;

    println!("Starting locald server...");
    let mut child = std::process::Command::new(server_bin)
        .arg("server")
        .arg("start")
        .env("RUST_LOG", "info")
        .stdout(server_log_file.try_clone()?)
        .stderr(server_log_file)
        .spawn()?;

    std::thread::sleep(std::time::Duration::from_secs(2));

    println!("Adding exec service...");
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
    let pids = util::process::find_pids_matching(|p| util::process::cmd_any_contains(p, "locald"));
    let _ = util::process::kill_pids(&pids, util::process::KillStrategy::TermThenKill);

    res
}
