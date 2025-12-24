use anyhow::Result;
use xshell::{cmd, Shell};

pub fn run(sh: &Shell) -> Result<()> {
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
