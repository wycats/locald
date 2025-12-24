use anyhow::Result;
use xshell::{Shell, cmd};

use crate::util;

pub fn build(sh: &Shell) -> Result<()> {
    println!("Building assets...");

    let server_assets = sh.current_dir().join("crates/locald-server/src/assets");
    let _ = sh.remove_path(&server_assets);
    sh.create_dir(&server_assets)?;

    // Dashboard
    println!("Building Dashboard...");
    cmd!(sh, "pnpm --filter locald-dashboard install").run()?;
    cmd!(sh, "pnpm --filter locald-dashboard build").run()?;

    // Copy dashboard assets
    // Check build/ or .svelte-kit/output/client/
    if sh.path_exists("locald-dashboard/build") {
        println!("Copying Dashboard build...");
        util::fs::copy_dir_recursive("locald-dashboard/build", &server_assets)?;
    } else if sh.path_exists("locald-dashboard/.svelte-kit/output/client") {
        println!("Copying Dashboard client output...");
        util::fs::copy_dir_recursive("locald-dashboard/.svelte-kit/output/client", &server_assets)?;
    } else {
        return Err(anyhow::anyhow!("Could not find dashboard build output"));
    }

    // Docs
    println!("Building Docs...");
    cmd!(sh, "pnpm --filter locald-docs install").run()?;
    cmd!(sh, "pnpm --filter locald-docs build").run()?;

    if sh.path_exists("locald-docs/dist") {
        println!("Copying Docs build...");
        let docs_out = server_assets.join("docs");
        sh.create_dir(&docs_out)?;
        util::fs::copy_dir_recursive("locald-docs/dist", &docs_out)?;
    } else {
        return Err(anyhow::anyhow!("Could not find docs build output"));
    }

    println!("Assets updated successfully in {}", server_assets.display());
    Ok(())
}
