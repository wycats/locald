//! Example locald plugin: Redis service provider
//!
//! This plugin demonstrates how to create a locald plugin that provides
//! Redis service configuration. When a service has `kind = "redis"`,
//! this plugin generates a plan to run Redis in a container.

// Generate bindings from the WIT interface
wit_bindgen::generate!({
    path: "../../crates/locald-server/wit",
    world: "locald-plugin",
});

use exports::locald::plugins::plugin::Guest;
use locald::plugins::types::{
    AllocatePortOp, DeclareServiceOp, Diagnostics, Expr, HostCapabilities, Op, Plan,
    ServiceSpec, Step, Value, WorkspaceContext,
};

struct Component;

impl Guest for Component {
    /// Detect if this plugin can handle the given service.
    ///
    /// Returns `Some(version)` if the service kind is "redis",
    /// indicating the plugin can handle it.
    fn detect(_ctx: WorkspaceContext, spec: ServiceSpec) -> Option<String> {
        if spec.kind == "redis" {
            // Extract version from config, default to "7"
            let version = spec
                .config
                .iter()
                .find(|(k, _)| k == "version")
                .and_then(|(_, v)| match v {
                    Value::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "7".to_string());

            Some(format!("redis:{version}"))
        } else {
            None
        }
    }

    /// Apply the plugin to generate a service plan.
    ///
    /// This creates a plan with two steps:
    /// 1. Allocate a port for Redis
    /// 2. Declare a container service running Redis
    fn apply(
        _ctx: WorkspaceContext,
        _caps: HostCapabilities,
        spec: ServiceSpec,
    ) -> Result<Plan, Diagnostics> {
        if spec.kind != "redis" {
            return Err(Diagnostics {
                warnings: vec![],
                errors: vec![format!(
                    "redis-plugin cannot handle kind '{}', expected 'redis'",
                    spec.kind
                )],
            });
        }

        // Extract version from config, default to "7"
        let version = spec
            .config
            .iter()
            .find(|(k, _)| k == "version")
            .and_then(|(_, v)| match v {
                Value::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "7".to_string());

        let image = format!("redis:{version}");
        let port_step_id = format!("{}_port", spec.name);

        // Step 1: Allocate a port for Redis
        let allocate_port_step = Step {
            id: port_step_id.clone(),
            needs: vec![],
            op: Op::AllocatePort(AllocatePortOp {
                name: spec.name.clone(),
            }),
        };

        // Step 2: Declare the container service
        // Use the allocated port from step 1
        let declare_service_step = Step {
            id: format!("{}_service", spec.name),
            needs: vec![port_step_id.clone()],
            op: Op::DeclareService(DeclareServiceOp {
                name: spec.name.clone(),
                runtime: "container".to_string(),
                settings: vec![
                    ("image".to_string(), Expr::Lit(Value::Text(image))),
                    (
                        "container_port".to_string(),
                        Expr::Lit(Value::Unsigned(6379)),
                    ),
                ],
            }),
        };

        Ok(Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![allocate_port_step, declare_service_step],
        })
    }
}

export!(Component);
