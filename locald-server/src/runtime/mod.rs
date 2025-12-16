pub mod docker;
pub mod process;

use self::docker::DockerRuntime;
use self::process::ProcessRuntime;
use bollard::Docker;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Runtime {
    pub docker: DockerRuntime,
    pub process: ProcessRuntime,
}

impl Runtime {
    #[must_use]
    pub fn new(docker: Arc<Docker>, notify_socket_path: PathBuf) -> Self {
        Self {
            docker: DockerRuntime::new(docker),
            process: ProcessRuntime::new(notify_socket_path),
        }
    }
}
