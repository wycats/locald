use anyhow::Result;
use xshell::{cmd, Shell};

pub fn build(sh: &Shell, args: Vec<String>) -> Result<()> {
    setup_env(sh)?;
    let mut cmd = cmd!(sh, "cargo build");
    for arg in args {
        cmd = cmd.arg(arg);
    }
    cmd.run()?;
    Ok(())
}

pub fn clippy(sh: &Shell, args: Vec<String>) -> Result<()> {
    setup_env(sh)?;
    let mut cmd = cmd!(sh, "cargo clippy");
    for arg in args {
        cmd = cmd.arg(arg);
    }
    cmd.run()?;
    Ok(())
}

fn setup_env(sh: &Shell) -> Result<()> {
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
