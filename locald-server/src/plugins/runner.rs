use anyhow::{Context, Result};
use std::path::Path;

use wasmtime::{Config, Engine, Store, component::Component, component::Linker};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView, add_to_linker_sync};

// Generated bindings for the WIT contract.
//
// Note: the contract is intentionally small for Phase 29. Plugins are plan transforms.
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/locald-plugin.wit",
        world: "locald-plugin",
    });
}

pub use bindings::locald::plugins::types::{Diagnostics, Plan, Step};

pub use bindings::locald::plugins::types::{
    AllocatePortOp, DeclareServiceOp, Expr, Op, OutputRef, Selector, Value,
    WorkspaceContext as WitWorkspaceContext,
};

pub type PlanResult = std::result::Result<Plan, Diagnostics>;

#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    pub workspace_id: String,
    pub root: String,
}

#[derive(Debug, Clone)]
pub struct HostCapabilities {
    pub supported_ir_versions: Vec<u32>,
    pub granted: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub name: String,
    pub kind: String,
    pub depends_on: Vec<String>,
    pub config: Vec<(String, bindings::locald::plugins::types::Value)>,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginApplyError {
    #[error("plugin returned diagnostics: {0:?}")]
    Diagnostics(Diagnostics),
}

#[allow(missing_debug_implementations)]
pub struct PluginRunner {
    engine: Engine,
}

impl PluginRunner {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            // Fuel gives us a low-friction execution limiter. We can add epoch interruption
            // for wall-clock timeouts later when we have real workloads.
            .consume_fuel(true);

        let engine = Engine::new(&config).context("failed to create wasmtime engine")?;
        Ok(Self { engine })
    }

    pub fn apply(
        &self,
        component_path: &Path,
        ctx: WorkspaceContext,
        caps: HostCapabilities,
        spec: ServiceSpec,
    ) -> Result<PlanResult> {
        let component = Component::from_file(&self.engine, component_path).with_context(|| {
            format!(
                "failed to load plugin component {}",
                component_path.display()
            )
        })?;

        let mut linker = Linker::new(&self.engine);

        // WASI is intentionally minimal for Phase 29: WASI-only, with no preopened dirs by default.
        // We wire WASI in so plugin authors can use standard tooling, but we don't enable the broader
        // WASI cloud ecosystem (http/sockets/etc.) unless explicitly added later.
        let (wasi, table) = wasi_context();
        let mut store = Store::new(&self.engine, HostState { wasi, table });
        // A conservative fuel budget for plan transforms. This is not yet a contract; it's a safety rail.
        store.set_fuel(10_000_000)?;

        add_to_linker_sync(&mut linker)?;

        let plugin = bindings::LocaldPlugin::instantiate(&mut store, &component, &linker)
            .context("failed to instantiate locald-plugin component")?;

        let wit_ctx = bindings::locald::plugins::types::WorkspaceContext {
            workspace_id: ctx.workspace_id,
            root: ctx.root,
        };

        let wit_caps = bindings::locald::plugins::types::HostCapabilities {
            supported_ir_versions: caps.supported_ir_versions,
            granted: caps.granted,
        };

        let wit_spec = bindings::locald::plugins::types::ServiceSpec {
            name: spec.name,
            kind: spec.kind,
            depends_on: spec.depends_on,
            config: spec.config,
        };

        let result = plugin
            .locald_plugins_plugin()
            .call_apply(&mut store, &wit_ctx, &wit_caps, &wit_spec)
            .context("plugin.apply failed")?;

        Ok(result)
    }
}

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

fn wasi_context() -> (WasiCtx, ResourceTable) {
    let table = ResourceTable::new();

    // Intentionally minimal: no inherit_env/args, no preopened dirs, no stdio.
    // Plugins are expected to return structured diagnostics instead of printing.
    let wasi = WasiCtxBuilder::new().build();

    (wasi, table)
}
