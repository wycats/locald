use anyhow::Result;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub fn copy_dir_recursive(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    if !src.exists() {
        return Err(anyhow::anyhow!(
            "copy_dir_recursive: src does not exist: {}",
            src.display()
        ));
    }

    std::fs::create_dir_all(dst)?;

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let out = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }

        if entry.file_type().is_symlink() {
            #[cfg(unix)]
            {
                use std::os::unix::fs as unix_fs;
                let target = std::fs::read_link(entry.path())?;
                let _ = std::fs::remove_file(&out);
                unix_fs::symlink(target, &out)?;
            }
            #[cfg(not(unix))]
            {
                // Best-effort: skip symlinks on non-unix.
            }
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &out)?;
        }
    }

    Ok(())
}

pub fn bump_mtime(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let now = std::time::SystemTime::now();
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(now))?;
    Ok(())
}

pub fn read_tail_bytes(path: impl AsRef<Path>, max_bytes: usize) -> Result<String> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn is_excluded(rel: &Path) -> bool {
    // Exclude top-level or nested build artifacts commonly found in this repo.
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        matches!(
            s.as_ref(),
            ".git"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | ".svelte-kit"
                | ".turbo"
                | ".next"
        )
    })
}

pub fn sync_tree(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        if is_excluded(rel) {
            continue;
        }

        let out = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }

        if entry.file_type().is_symlink() {
            #[cfg(unix)]
            {
                use std::os::unix::fs as unix_fs;
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let target = std::fs::read_link(entry.path())?;
                let _ = std::fs::remove_file(&out);
                unix_fs::symlink(target, &out)?;
            }
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &out)?;
        }
    }

    Ok(())
}

pub fn write_file(path: impl AsRef<Path>, contents: &str) -> Result<()> {
    std::fs::write(path.as_ref(), contents)?;
    Ok(())
}
