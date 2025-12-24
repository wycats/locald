use anyhow::Result;
use xshell::{cmd, Shell};

use crate::{docs, util};

pub fn run(sh: &Shell, full: bool) -> Result<()> {
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

    let expected = std::fs::read_to_string("docs/surface/cli-manifest.json")?;
    let expected_json: serde_json::Value = serde_json::from_str(&expected)?;
    let actual_json: serde_json::Value = serde_json::from_str(&output)?;
    if expected_json != actual_json {
        let tmp = sh.create_temp_dir()?;
        let tmp_path = tmp.path().join("locald-cli-manifest.json");
        util::fs::write_file(&tmp_path, &output)?;
        println!("error: docs/surface/cli-manifest.json is out of date");
        println!("note: wrote current manifest to {}", tmp_path.display());
        return Err(anyhow::anyhow!("CLI surface manifest mismatch"));
    }

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
    docs::verify_docs(sh)?;

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
