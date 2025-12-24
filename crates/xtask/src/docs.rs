use anyhow::Result;
use xshell::Shell;

// --- Documentation Verification ---

#[derive(serde::Deserialize)]
struct Manifest {
    root: CommandNode,
}

#[derive(serde::Deserialize, Clone)]
struct CommandNode {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    args: Vec<ArgNode>,
    #[serde(default)]
    subcommands: Vec<CommandNode>,
}

#[derive(serde::Deserialize, Clone)]
struct ArgNode {
    long: Option<String>,
    short: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    positional: bool,
}

pub fn verify_docs(sh: &Shell) -> Result<()> {
    println!("Checking documentation...");

    // Check active RFCs
    let rfc_dir = sh.current_dir().join("docs/rfcs");
    let mut active_rfcs = 0;
    if rfc_dir.exists() {
        for entry in std::fs::read_dir(rfc_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let content = std::fs::read_to_string(&path)?;
                if content.lines().any(|l| l.starts_with("stage: 2")) {
                    active_rfcs += 1;
                }
            }
        }
    }

    if active_rfcs == 0 {
        println!("Note: No active RFCs (Stage 2: Available) found.");
        println!("If you are implementing a feature, ensure you have an RFC in Stage 2.");
    } else {
        println!("Found {} active RFC(s).", active_rfcs);
    }

    // Check plan-outline.md
    if !sh.path_exists("docs/agent-context/plan-outline.md") {
        println!("Warning: docs/agent-context/plan-outline.md not found.");
    }

    verify_docs_screenshots(sh)?;
    verify_docs_sidebar(sh)?;
    verify_docs_cli(sh)?;

    println!("Documentation checks passed.");
    Ok(())
}

fn verify_docs_screenshots(sh: &Shell) -> Result<()> {
    println!("Checking screenshots...");
    let docs_root = sh.current_dir().join("locald-docs/src/content/docs");
    let screenshots_root = sh.current_dir().join("locald-docs/public/screenshots");

    if !docs_root.exists() || !screenshots_root.exists() {
        println!("Skipping screenshot check (dirs not found)");
        return Ok(());
    }

    let mut referenced = std::collections::HashSet::new();
    let re = regex::Regex::new(r"/(screenshots/[^\s)\]\x22']+\.png)")?;

    for entry in walkdir::WalkDir::new(&docs_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "md" || ext == "mdx" {
                let content = std::fs::read_to_string(path)?;
                for cap in re.captures_iter(&content) {
                    referenced.insert(cap[1].to_string());
                }
            }
        }
    }

    let mut missing = Vec::new();
    for rel in &referenced {
        let file_name = rel.strip_prefix("screenshots/").unwrap_or(rel);
        let path = screenshots_root.join(file_name);
        if !path.exists() {
            missing.push(rel.clone());
        }
    }

    if !missing.is_empty() {
        println!("Missing screenshots:");
        for m in missing {
            println!("  - {}", m);
        }
        return Err(anyhow::anyhow!("Missing screenshots found"));
    }

    // Check for unused
    let mut unused = Vec::new();
    for entry in walkdir::WalkDir::new(&screenshots_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy();
        let rel = format!("screenshots/{}", file_name);
        if !referenced.contains(&rel) {
            unused.push(rel);
        }
    }

    if !unused.is_empty() {
        println!("Unused screenshots (warning):");
        for u in unused {
            println!("  - {}", u);
        }
    }

    Ok(())
}

fn verify_docs_sidebar(sh: &Shell) -> Result<()> {
    println!("Checking sidebar links...");
    let config_path = sh.current_dir().join("locald-docs/astro.config.mjs");
    if !config_path.exists() {
        println!("Skipping sidebar check (config not found)");
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let re = regex::Regex::new(r"\blink\s*:\s*['\x22]([^'\x22]+)['\x22]")?;

    let mut seen = std::collections::HashMap::new();
    let mut dups = std::collections::HashMap::new();

    for cap in re.captures_iter(&content) {
        let link = &cap[1];
        let normalized = if link == "/" {
            "/".to_string()
        } else {
            let l = if !link.starts_with('/') {
                format!("/{}", link)
            } else {
                link.to_string()
            };
            if l.ends_with('/') {
                l
            } else {
                format!("{}/", l)
            }
        };

        if let Some(count) = seen.get_mut(&normalized) {
            *count += 1;
            dups.insert(normalized.clone(), *count);
        } else {
            seen.insert(normalized, 1);
        }
    }

    if !dups.is_empty() {
        println!("Duplicate sidebar links detected:");
        for (link, count) in dups {
            println!("  - {} (occurrences: {})", link, count);
        }
        return Err(anyhow::anyhow!("Duplicate sidebar links found"));
    }

    Ok(())
}

fn verify_docs_cli(sh: &Shell) -> Result<()> {
    println!("Checking CLI surface docs...");
    let manifest_path = sh.current_dir().join("docs/surface/cli-manifest.json");
    if !manifest_path.exists() {
        println!("Skipping CLI check (manifest not found)");
        return Ok(());
    }

    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let root = manifest.root;

    // Build global index
    let mut global_long = std::collections::HashSet::new();
    let mut global_short = std::collections::HashSet::new();
    for arg in &root.args {
        if arg.global && !arg.positional {
            if let Some(l) = &arg.long {
                global_long.insert(l.clone());
            }
            if let Some(s) = &arg.short {
                global_short.insert(s.clone());
            }
            for a in &arg.aliases {
                global_long.insert(a.clone());
            }
        }
    }

    let docs_root = sh.current_dir().join("locald-docs/src/content/docs");
    let readme = sh.current_dir().join("README.md");

    let mut files = Vec::new();
    if readme.exists() {
        files.push(readme);
    }
    if docs_root.exists() {
        for entry in walkdir::WalkDir::new(&docs_root) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "md" || ext == "mdx" {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
    }

    let mut errors = Vec::new();

    for file_path in files {
        let content = std::fs::read_to_string(&file_path)?;
        let invocations = extract_locald_invocations(&content);

        for inv in invocations {
            if let Err(e) = validate_invocation(&root, &global_long, &global_short, &inv.tokens) {
                errors.push(format!(
                    "{}:{}: {} (Snippet: {})",
                    file_path.display(),
                    inv.line,
                    e,
                    inv.tokens.join(" ")
                ));
            }
        }
    }

    if !errors.is_empty() {
        println!("CLI surface docs lint failed:");
        for e in errors {
            println!("- {}", e);
        }
        return Err(anyhow::anyhow!("CLI surface docs lint failed"));
    }

    Ok(())
}

struct Invocation {
    line: usize,
    tokens: Vec<String>,
}

fn extract_locald_invocations(content: &str) -> Vec<Invocation> {
    let mut invocations = Vec::new();
    let mut in_fence = false;
    let mut fence_lang = String::new();

    for (i, line) in content.lines().enumerate() {
        let line_trim = line.trim();
        if line_trim.starts_with("```") {
            if !in_fence {
                in_fence = true;
                fence_lang = line_trim.trim_start_matches("```").trim().to_lowercase();
            } else {
                in_fence = false;
                fence_lang.clear();
            }
            continue;
        }

        if !in_fence {
            continue;
        }

        if !fence_lang.is_empty()
            && !["bash", "sh", "shell", "zsh", "console", "terminal", ""]
                .contains(&fence_lang.as_str())
        {
            continue;
        }

        let cleaned = strip_comment(&strip_prompt(line)).trim().to_string();
        if cleaned.is_empty() {
            continue;
        }

        let mut tokens = shlex(&cleaned);
        if tokens.is_empty() {
            continue;
        }

        // Skip env vars
        while !tokens.is_empty() && tokens[0].contains('=') && !tokens[0].starts_with('-') {
            if regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=")
                .unwrap()
                .is_match(&tokens[0])
            {
                tokens.remove(0);
            } else {
                break;
            }
        }

        if tokens.is_empty() {
            continue;
        }
        if tokens[0] == "sudo" {
            tokens.remove(0);
        }
        if tokens.is_empty() {
            continue;
        }

        if tokens[0] != "locald" {
            continue;
        }

        invocations.push(Invocation {
            line: i + 1,
            tokens,
        });
    }
    invocations
}

fn validate_invocation(
    root: &CommandNode,
    global_long: &std::collections::HashSet<String>,
    global_short: &std::collections::HashSet<String>,
    tokens: &[String],
) -> Result<()> {
    let mut tokens = tokens.to_vec();
    tokens.remove(0); // consume locald

    let mut current = root;

    // Greedy descent
    while !tokens.is_empty() {
        let next = &tokens[0];
        if next.starts_with('-') {
            break;
        } // Flag

        // Find subcommand
        let mut found = None;
        for sub in &current.subcommands {
            if sub.name == *next || sub.aliases.contains(next) {
                found = Some(sub);
                break;
            }
        }

        if let Some(sub) = found {
            current = sub;
            tokens.remove(0);
        } else {
            break; // Positional or unknown
        }
    }

    // Validate flags
    for token in tokens {
        if token == "--" {
            break;
        }
        if !token.starts_with('-') {
            break;
        } // Stop at first non-flag (positional or value)

        if token.starts_with("--") {
            let name = token.trim_start_matches("--").split('=').next().unwrap();
            if global_long.contains(name) {
                continue;
            }

            let mut found = false;
            for arg in &current.args {
                if let Some(l) = &arg.long {
                    if l == name {
                        found = true;
                        break;
                    }
                }
                for a in &arg.aliases {
                    if a == name {
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err(anyhow::anyhow!("Unknown flag: --{}", name));
            }
        } else {
            // Short flags
            let cluster = token.trim_start_matches('-');
            for ch in cluster.chars() {
                let s = ch.to_string();
                if global_short.contains(&s) {
                    continue;
                }

                let mut found = false;
                for arg in &current.args {
                    if let Some(sh) = &arg.short {
                        if sh == &s {
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    return Err(anyhow::anyhow!("Unknown flag: -{}", ch));
                }
            }
        }
    }

    Ok(())
}

fn shlex(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' && !in_single {
            if let Some(next) = chars.next() {
                cur.push(next);
            }
            continue;
        }
        if !in_double && ch == '\'' {
            in_single = !in_single;
            continue;
        }
        if !in_single && ch == '"' {
            in_double = !in_double;
            continue;
        }
        if !in_single && !in_double && ch.is_whitespace() {
            if !cur.is_empty() {
                tokens.push(cur);
                cur = String::new();
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn strip_prompt(line: &str) -> String {
    let line = line.trim_start();
    if let Some(stripped) = line.strip_prefix("$ ") {
        stripped.to_string()
    } else {
        line.to_string()
    }
}

fn strip_comment(line: &str) -> String {
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = line.char_indices();

    while let Some((i, ch)) = chars.next() {
        if ch == '\\' {
            chars.next();
            continue;
        }
        if !in_double && ch == '\'' {
            in_single = !in_single;
        } else if !in_single && ch == '"' {
            in_double = !in_double;
        } else if !in_single && !in_double && ch == '#' {
            return line[..i].trim_end().to_string();
        }
    }
    line.to_string()
}
