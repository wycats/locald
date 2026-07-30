#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

struct CliTestSandbox {
    _root: tempfile::TempDir,
    socket: PathBuf,
    data: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl CliTestSandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create CLI test sandbox");
        Self {
            socket: root.path().join("locald.sock"),
            data: root.path().join("data"),
            config: root.path().join("config"),
            state: root.path().join("state"),
            _root: root,
        }
    }

    fn configure_cases(&self, cases: &trycmd::TestCases) {
        cases
            .env("LOCALD_SANDBOX_ACTIVE", "1")
            .env("LOCALD_SANDBOX_NAME", "cli-snapshots")
            .env("LOCALD_SOCKET", path_env(&self.socket))
            .env("LOCALD_HTTP_PORT", "0")
            .env("LOCALD_HTTPS_PORT", "0")
            .env("XDG_DATA_HOME", path_env(&self.data))
            .env("XDG_CONFIG_HOME", path_env(&self.config))
            .env("XDG_STATE_HOME", path_env(&self.state));
    }

    fn shutdown(&self) {
        if !self.socket.exists() {
            return;
        }

        let status = self
            .shutdown_command()
            .status()
            .expect("run CLI test daemon shutdown");
        assert!(status.success(), "CLI test daemon shutdown failed");

        for _ in 0..50 {
            if !self.socket.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }

        panic!("CLI test daemon socket remained after shutdown");
    }

    fn shutdown_command(&self) -> Command {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
        command
            .args(["server", "shutdown"])
            .env("LOCALD_SANDBOX_ACTIVE", "1")
            .env("LOCALD_SANDBOX_NAME", "cli-snapshots")
            .env("LOCALD_SOCKET", &self.socket)
            .env("LOCALD_HTTP_PORT", "0")
            .env("LOCALD_HTTPS_PORT", "0")
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_STATE_HOME", &self.state);
        command
    }
}

impl Drop for CliTestSandbox {
    fn drop(&mut self) {
        if self.socket.exists() {
            let _ = self.shutdown_command().status();
        }
    }
}

fn path_env(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn cli_tests() {
    let sandbox = CliTestSandbox::new();
    let t = trycmd::TestCases::new();
    sandbox.configure_cases(&t);

    t.case("tests/cmd/ai-schema.md");
    t.case("tests/cmd/docs-cli.md");
    t.case("tests/cmd/error-messages.md");
    t.case("tests/cmd/help-subcommands.md");
    t.case("tests/cmd/version.md");

    if cfg!(feature = "experimental-cnb")
        || cfg!(feature = "experimental-containers")
        || cfg!(feature = "experimental-plugins")
        || cfg!(feature = "experimental-vmm")
    {
        t.case("tests/cmd/help-nightly.md");
    } else {
        t.case("tests/cmd/help.md");
    }

    t.run();
    sandbox.shutdown();
}
