use anyhow::Result;
use xshell::{cmd, Shell};

pub fn watch(
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

pub fn logs(sh: &Shell, watch: bool) -> Result<()> {
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

pub fn tripwire(sh: &Shell, base: String) -> Result<()> {
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
