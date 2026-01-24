pub mod process;

use self::process::ProcessRuntime;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Runtime {
    pub process: ProcessRuntime,
}

impl Runtime {
    #[must_use]
    pub fn new(notify_socket_path: PathBuf) -> Self {
        Self {
            process: ProcessRuntime::new(notify_socket_path),
        }
    }
}
