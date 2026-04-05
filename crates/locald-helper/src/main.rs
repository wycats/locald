//! macOS privileged helper daemon for locald.
//!
//! Runs as a `LaunchDaemon` and performs operations that require root:
//! pfctl port forwarding and CA trust installation. Communicates with
//! the locald agent via XPC (Mach service `com.locald.helper`).

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    #[allow(clippy::print_stdout)]
    {
        println!("locald-helper is macOS-only");
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context, Result};
    use futures::stream::StreamExt;
    use std::collections::HashMap;
    use std::ffi::CString;
    use xpc_connection::{Message, XpcClient, XpcListener};

    const MACH_SERVICE: &str = "com.locald.helper";
    // ── pfctl constants ─────────────────────────────────────────────────

    const ANCHOR_FILE: &str = "/etc/pf.anchors/com.locald";
    const PF_CONF: &str = "/etc/pf.conf";
    const RDR_ANCHOR_LINE: &str = "rdr-anchor \"com.locald\"";
    const LOAD_ANCHOR_LINE: &str = "load anchor \"com.locald\" from \"/etc/pf.anchors/com.locald\"";

    struct PortForward {
        from: u16,
        to: u16,
    }

    const FORWARDS: &[PortForward] = &[
        PortForward { from: 80, to: 8080 },
        PortForward {
            from: 443,
            to: 8443,
        },
    ];

    // ── entry point ─────────────────────────────────────────────────────

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(serve())?;
        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn serve() -> Result<()> {
        #[allow(clippy::expect_used)]
        let name = CString::new(MACH_SERVICE).expect("static CString");
        let mut listener = XpcListener::listen(&name);

        while let Some(client) = listener.next().await {
            tokio::spawn(handle_client(client));
        }

        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn handle_client(mut client: XpcClient) {
        // Validate caller: reject connections from root (uid 0).
        // Only the console user's agent should be calling us.
        // audit_token_t layout: [auid, euid, egid, ruid, rgid, pid, asid, pidver]
        // Each field is u32. We want euid (field 1, bytes 4-7).
        let token = client.audit_token();
        let caller_uid = u32::from_le_bytes([token[4], token[5], token[6], token[7]]);
        if caller_uid == 0 {
            client.send_message(error_response("rejected: caller is root"));
            return;
        }

        loop {
            match client.next().await {
                None | Some(Message::Error(_)) => break,
                Some(Message::Dictionary(dict)) => {
                    let response = handle_command(&dict, caller_uid);
                    client.send_message(response);
                }
                Some(_) => {
                    client.send_message(error_response("expected dictionary message"));
                }
            }
        }
    }

    fn handle_command(dict: &HashMap<CString, Message>, caller_uid: u32) -> Message {
        #[allow(clippy::expect_used)]
        let command_key = CString::new("command").expect("static CString");

        let command = match dict.get(&command_key) {
            Some(Message::String(s)) => s.to_string_lossy().to_string(),
            _ => return error_response("missing or invalid 'command' field"),
        };

        match command.as_str() {
            "setup" => execute_setup(caller_uid),
            _ => error_response(&format!("unknown command: {command}")),
        }
    }

    fn execute_setup(caller_uid: u32) -> Message {
        if let Err(e) = do_setup(caller_uid) {
            return error_response(&format!("{e:#}"));
        }
        success_response()
    }

    fn do_setup(caller_uid: u32) -> Result<()> {
        // 1. Install pfctl redirect rules (runtime + persistent).
        apply_port_forwarding().context("pfctl install failed")?;

        // 3. Trust the Root CA in the system keychain (for the calling user).
        trust_root_ca(caller_uid).context("CA trust failed")?;

        // 4. Update global config to enable privileged ports.
        update_privileged_ports_config(caller_uid)
            .context("config update failed (non-fatal)")
            .ok();

        Ok(())
    }

    // ── pfctl ───────────────────────────────────────────────────────────

    fn generate_anchor_rules() -> String {
        use std::fmt::Write;

        let mut rules = String::new();
        for f in FORWARDS {
            if let Err(e) = writeln!(
                rules,
                "rdr pass on lo0 proto tcp from any to 127.0.0.1 port {} -> 127.0.0.1 port {}",
                f.from, f.to
            ) {
                tracing::warn!("Failed to write pf anchor rule: {e}");
            }
        }

        rules
    }

    /// Apply port forwarding rules. Writes the anchor file, ensures pf.conf
    /// references it, enables pf, and reloads from disk. Idempotent.
    #[allow(clippy::disallowed_methods)]
    fn apply_port_forwarding() -> Result<()> {
        // 1. Write anchor file
        std::fs::write(ANCHOR_FILE, generate_anchor_rules())
            .context("Failed to write /etc/pf.anchors/com.locald")?;

        // 2. Ensure pf.conf references our anchor
        let content = std::fs::read_to_string(PF_CONF).context("Failed to read /etc/pf.conf")?;

        let has_rdr = content.lines().any(|l| l.trim() == RDR_ANCHOR_LINE);
        let has_load = content.lines().any(|l| l.trim() == LOAD_ANCHOR_LINE);

        if !has_rdr || !has_load {
            let mut lines: Vec<String> = content.lines().map(String::from).collect();

            if !has_rdr {
                let pos = lines
                    .iter()
                    .rposition(|l| l.trim().starts_with("rdr-anchor"));
                let insert_at = pos.map_or(lines.len(), |i| i + 1);
                lines.insert(insert_at, RDR_ANCHOR_LINE.to_string());
            }

            if !has_load {
                let pos = lines
                    .iter()
                    .rposition(|l| l.trim().starts_with("load anchor"));
                let insert_at = pos.map_or(lines.len(), |i| i + 1);
                lines.insert(insert_at, LOAD_ANCHOR_LINE.to_string());
            }

            let mut new_content = lines.join("\n");
            if !new_content.ends_with('\n') {
                new_content.push('\n');
            }

            let tmp = format!("{PF_CONF}.locald.tmp");
            std::fs::write(&tmp, &new_content).context("Failed to write temporary pf.conf")?;
            std::fs::rename(&tmp, PF_CONF).context("Failed to rename temporary pf.conf")?;
        }

        // 3. Enable pf (idempotent)
        if let Err(e) = std::process::Command::new("pfctl").args(["-e"]).output() {
            tracing::warn!("pfctl -e failed: {e}");
        }

        // 4. Reload from disk — this is the single activation path
        let output = std::process::Command::new("pfctl")
            .args(["-f", PF_CONF])
            .output()
            .context("Failed to run pfctl -f")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("Use of -f option") && !stderr.contains("ALTQ") {
                anyhow::bail!("pfctl -f failed: {stderr}");
            }
        }

        Ok(())
    }

    // ── CA trust ────────────────────────────────────────────────────────

    /// Trust the locald Root CA in the system keychain.
    ///
    /// The helper runs as root, so the CA cert lives in the invoking user's
    /// home directory. We resolve the cert path from the caller's UID
    /// (obtained from the XPC audit token).
    #[allow(clippy::disallowed_methods)]
    fn trust_root_ca(caller_uid: u32) -> Result<()> {
        let ca_path = resolve_user_ca_path(caller_uid)?;

        if !ca_path.exists() {
            anyhow::bail!(
                "Root CA not found at {}. Run `locald admin trust` first.",
                ca_path.display()
            );
        }

        let status = std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot", "-k"])
            .arg("/Library/Keychains/System.keychain")
            .arg(&ca_path)
            .status()
            .context("Failed to execute `security add-trusted-cert`")?;

        if !status.success() {
            anyhow::bail!("`security add-trusted-cert` failed with status: {status}");
        }

        Ok(())
    }

    /// Resolve the Root CA path for the calling user (identified by UID
    /// from the XPC audit token).
    fn resolve_user_ca_path(caller_uid: u32) -> Result<std::path::PathBuf> {
        let uid = nix::unistd::Uid::from_raw(caller_uid);
        let user = nix::unistd::User::from_uid(uid)
            .context("Failed to look up user by UID")?
            .context("User not found for UID")?;

        let ca_path = user
            .dir
            .join("Library")
            .join("Application Support")
            .join("locald")
            .join("certs")
            .join("rootCA.pem");

        Ok(ca_path)
    }

    /// Update the global config to enable privileged ports.
    ///
    /// The config file lives in the calling user's data dir, so we resolve
    /// it from their UID.
    #[allow(clippy::disallowed_methods)]
    fn update_privileged_ports_config(caller_uid: u32) -> Result<()> {
        let uid = nix::unistd::Uid::from_raw(caller_uid);
        let user = nix::unistd::User::from_uid(uid)
            .context("Failed to look up user by UID")?
            .context("User not found for UID")?;

        let config_path = user
            .dir
            .join("Library")
            .join("Application Support")
            .join("locald")
            .join("config.toml");

        // Best-effort config update. Intentionally non-fatal (called with .ok()).
        let content = std::fs::read_to_string(&config_path).unwrap_or_default();
        let new_content = if content.contains("privileged_ports = true") {
            // Already set correctly.
            return Ok(());
        } else if content.contains("privileged_ports") {
            // Has the key but not set to true — flip it.
            content.replace("privileged_ports = false", "privileged_ports = true")
        } else if content.contains("[server]") {
            // Has [server] section but no key.
            content.replace("[server]", "[server]\nprivileged_ports = true")
        } else {
            // No server section at all.
            format!("{content}\n[server]\nprivileged_ports = true\n")
        };

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        }
        std::fs::write(&config_path, &new_content).context("Failed to write config")?;

        // Chown the config to the calling user so they can modify it later.
        let gid = nix::unistd::Gid::from_raw(user.gid.as_raw());
        if let Err(e) = nix::unistd::chown(&config_path, Some(uid), Some(gid)) {
            tracing::warn!("Failed to chown config: {e}");
        }

        Ok(())
    }

    // ── XPC response helpers ────────────────────────────────────────────

    fn success_response() -> Message {
        let mut dict = HashMap::new();
        #[allow(clippy::expect_used)]
        {
            dict.insert(
                CString::new("status").expect("static CString"),
                Message::String(CString::new("success").expect("static CString")),
            );
        }
        Message::Dictionary(dict)
    }

    fn error_response(msg: &str) -> Message {
        let mut dict = HashMap::new();
        #[allow(clippy::expect_used)]
        {
            dict.insert(
                CString::new("status").expect("static CString"),
                Message::String(CString::new("error").expect("static CString")),
            );
            dict.insert(
                CString::new("message").expect("static CString"),
                Message::String(CString::new(msg.replace('\0', "")).expect("sanitized CString")),
            );
        }
        Message::Dictionary(dict)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn make_command_dict(command: &str) -> HashMap<CString, Message> {
            let mut dict = HashMap::new();
            dict.insert(
                CString::new("command").unwrap(),
                Message::String(CString::new(command).unwrap()),
            );
            dict
        }

        fn get_status(msg: &Message) -> String {
            match msg {
                Message::Dictionary(d) => {
                    let key = CString::new("status").unwrap();
                    match d.get(&key) {
                        Some(Message::String(s)) => s.to_string_lossy().to_string(),
                        _ => panic!("no status field"),
                    }
                }
                _ => panic!("expected dictionary"),
            }
        }

        fn get_message_field(msg: &Message) -> String {
            match msg {
                Message::Dictionary(d) => {
                    let key = CString::new("message").unwrap();
                    match d.get(&key) {
                        Some(Message::String(s)) => s.to_string_lossy().to_string(),
                        _ => panic!("no message field"),
                    }
                }
                _ => panic!("expected dictionary"),
            }
        }

        #[test]
        fn unknown_command_returns_error() {
            let dict = make_command_dict("bogus");
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
            assert!(get_message_field(&resp).contains("unknown command: bogus"));
        }

        #[test]
        fn missing_command_field_returns_error() {
            let dict = HashMap::new();
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
            assert!(get_message_field(&resp).contains("missing or invalid"));
        }

        #[test]
        fn wrong_command_type_returns_error() {
            let mut dict = HashMap::new();
            dict.insert(CString::new("command").unwrap(), Message::Int64(42));
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
        }

        #[test]
        fn success_response_has_correct_format() {
            let resp = success_response();
            assert_eq!(get_status(&resp), "success");
        }

        #[test]
        fn error_response_strips_null_bytes() {
            let resp = error_response("bad\0data");
            assert_eq!(get_message_field(&resp), "baddata");
        }
    }
}
