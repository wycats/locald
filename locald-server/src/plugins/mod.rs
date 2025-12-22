use anyhow::Result;
use std::path::Path;

pub mod plan;
pub mod runner;

pub use runner::{HostCapabilities, PluginApplyError, PluginRunner, ServiceSpec, WorkspaceContext};

pub use plan::{apply_plan_to_config, validate_plan};

/// Convenience API for one-shot plugin apply.
pub fn apply_plugin(
    component_path: &Path,
    ctx: WorkspaceContext,
    caps: HostCapabilities,
    spec: ServiceSpec,
) -> Result<runner::PlanResult> {
    PluginRunner::new()?.apply(component_path, ctx, caps, spec)
}
