//! Shared, dependency-free rendering for locald-managed hosts-file sections.

const START_MARKER: &str = "# BEGIN locald";
const END_MARKER: &str = "# END locald";

/// Replace locald's managed hosts-file section with the supplied domains.
///
/// An empty domain set removes an existing complete managed section and leaves
/// an unclosed section unchanged. With a nonempty domain set, an unclosed
/// section is retained and a complete replacement is appended, matching
/// locald's historical fail-safe behavior.
#[must_use]
pub fn update_hosts_content(current_content: &str, domains: &[String]) -> String {
    if domains.is_empty() {
        let Some(start) = current_content.find(START_MARKER) else {
            return current_content.to_owned();
        };
        let Some(relative_end) = current_content[start..].find(END_MARKER) else {
            return current_content.to_owned();
        };
        let end = start + relative_end + END_MARKER.len();
        let mut output = String::from(&current_content[..start]);
        output.push_str(&current_content[end..]);
        return collapse_newline_runs(&output);
    }

    let mut new_section = String::new();
    new_section.push_str(START_MARKER);
    new_section.push('\n');
    for domain in domains {
        new_section.push_str("127.0.0.1 ");
        new_section.push_str(domain);
        new_section.push('\n');
    }
    new_section.push_str(END_MARKER);

    if let Some(start) = current_content.find(START_MARKER)
        && let Some(end_idx) = current_content[start..].find(END_MARKER)
    {
        let end = start + end_idx;
        let mut output = String::from(&current_content[..start]);
        output.push_str(&new_section);
        output.push_str(&current_content[end + END_MARKER.len()..]);
        return output;
    }

    let mut output = String::from(current_content);
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&new_section);
    output.push('\n');
    output
}

fn collapse_newline_runs(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut consecutive_newlines = 0;

    for character in content.chars() {
        if character == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines > 2 {
                continue;
            }
        } else {
            consecutive_newlines = 0;
        }
        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::update_hosts_content;

    #[test]
    fn empty_domains_remove_an_existing_section_idempotently() {
        let current =
            "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.localhost\n# END locald\n";

        let updated = update_hosts_content(current, &[]);

        assert_eq!(updated, "127.0.0.1 localhost\n\n");
        assert_eq!(update_hosts_content(&updated, &[]), updated);
    }

    #[test]
    fn empty_domains_collapse_long_newline_runs_in_one_pass() {
        let current = format!(
            "before\n# BEGIN locald\n127.0.0.1 old.localhost\n# END locald{}after\n",
            "\n".repeat(10_000)
        );

        let updated = update_hosts_content(&current, &[]);

        assert_eq!(updated, "before\n\nafter\n");
    }

    #[test]
    fn nonempty_domains_replace_an_existing_section() {
        let current =
            "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.localhost\n# END locald\n";

        let updated = update_hosts_content(current, &["custom.example.test".to_owned()]);

        assert!(!updated.contains("old.localhost"));
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert_eq!(updated.matches("# BEGIN locald").count(), 1);
    }
}
