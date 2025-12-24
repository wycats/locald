use anyhow::Result;
use xshell::{Shell, cmd};

pub fn run(sh: &Shell, sandbox: String, args: Vec<String>) -> Result<()> {
    println!("📦 Building locald...");
    cmd!(sh, "cargo build").run()?;

    let locald = sh.current_dir().join("target/debug/locald");
    let log_file = sh.current_dir().join("server.log");

    println!("🚀 Starting locald in sandbox '{}'...", sandbox);
    println!("   Log file: {}", log_file.display());

    // Start server in background
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
