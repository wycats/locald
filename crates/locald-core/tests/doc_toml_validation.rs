use locald_core::config::LocaldConfig;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn docs_toml_blocks_parse() {
    let docs_root = workspace_root()
        .join("locald-docs")
        .join("src")
        .join("content")
        .join("docs");

    let mut errors = Vec::new();
    let mut total_blocks = 0usize;

    for entry in walk_docs(&docs_root) {
        let path = entry;
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                errors.push(format!("{}: failed to read file: {err}", path.display()));
                continue;
            }
        };

        for block in extract_toml_blocks(&contents) {
            total_blocks += 1;
            let toml_str = block.content.trim();
            if toml_str.is_empty() {
                continue;
            }

            if let Err(err) = toml::from_str::<toml::Value>(toml_str) {
                errors.push(format!(
                    "{}:{}: TOML parse failed: {err}\n{toml_str}",
                    path.display(),
                    block.start_line
                ));
                continue;
            }

            if block.requires_full_config {
                if let Err(err) = toml::from_str::<LocaldConfig>(toml_str) {
                    errors.push(format!(
                        "{}:{}: LocaldConfig parse failed: {err}\n{toml_str}",
                        path.display(),
                        block.start_line
                    ));
                }
            }
        }
    }

    if total_blocks == 0 {
        errors.push(format!(
            "No TOML blocks found under {}",
            docs_root.display()
        ));
    }

    if !errors.is_empty() {
        panic!(
            "Documentation TOML validation failed:\n{}",
            errors.join("\n\n")
        );
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("workspace root")
}

fn walk_docs(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let path_str = path.to_string_lossy();
        if path_str.contains("/internals/rfcs") {
            continue;
        }
        if path.is_dir() {
            files.extend(walk_docs(&path));
        } else if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            if matches!(ext, "md" | "mdx") {
                files.push(path);
            }
        }
    }

    files
}

#[derive(Debug)]
struct TomlBlock {
    start_line: usize,
    content: String,
    requires_full_config: bool,
}

fn extract_toml_blocks(contents: &str) -> Vec<TomlBlock> {
    let mut blocks = Vec::new();
    let mut in_toml = false;
    let mut start_line = 0usize;
    let mut buffer = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();

        if !in_toml && trimmed.starts_with("```toml") {
            in_toml = true;
            start_line = index + 1;
            buffer.clear();
            continue;
        }

        if in_toml && trimmed.starts_with("```") {
            let content = buffer.join("\n");
            let requires_full_config =
                content.contains("[project]") || content.contains("project =");
            blocks.push(TomlBlock {
                start_line,
                content,
                requires_full_config,
            });
            in_toml = false;
            buffer.clear();
            continue;
        }

        if in_toml {
            buffer.push(line.to_string());
        }
    }

    blocks
}
