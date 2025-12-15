#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn shim_must_be_leaf_node() {
        // RFC 0096: The locald-shim binary MUST NEVER execute the locald binary.
        // This test scans the source code to ensure no `Command::new` calls reference "locald".

        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let src_dir = Path::new(&manifest_dir).join("src");

        visit_dirs(&src_dir, &|entry| {
            let content = fs::read_to_string(entry.path()).unwrap();

            // We look for Command::new(...) where the argument might be locald
            // This is a heuristic, but effective for preventing the specific anti-pattern.
            // We specifically look for the variable name `locald_path` which was used in the bad code.

            if content.contains("Command::new(&locald_path)") {
                panic!(
                    "Violation of RFC 0096 in {:?}: Shim must not exec locald. Found 'Command::new(&locald_path)'.",
                    entry.path()
                );
            }

            if content.contains("Command::new(\"locald\")") {
                panic!(
                    "Violation of RFC 0096 in {:?}: Shim must not exec locald. Found 'Command::new(\"locald\")'.",
                    entry.path()
                );
            }

            // Also check for the fallback logic variable
            if content.contains("let locald_path =") {
                 panic!(
                    "Violation of RFC 0096 in {:?}: Shim must not resolve locald path. Found 'let locald_path ='.",
                    entry.path()
                );
            }
        }).unwrap();
    }

    fn visit_dirs(dir: &Path, cb: &dyn Fn(&fs::DirEntry)) -> std::io::Result<()> {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    visit_dirs(&path, cb)?;
                } else {
                    cb(&entry);
                }
            }
        }
        Ok(())
    }
}
