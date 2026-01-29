use anyhow::Result;
use std::path::Path;

use serde_json::json;

use crate::port_allocator::{PortAllocator, PortGuard};

pub mod discovery;
pub mod plan;
pub mod runner;

pub use discovery::{default_capabilities, discover_plugins, user_plugins_dir};
pub use runner::{HostCapabilities, PluginApplyError, PluginRunner, ServiceSpec, WorkspaceContext};

pub use plan::{StepOutputs, apply_plan_to_config, validate_plan};

#[must_use]
pub fn normalized_plan_debug_json(plan: &runner::Plan) -> serde_json::Value {
    fn value_to_json(v: &runner::Value) -> serde_json::Value {
        match v {
            runner::Value::Null => json!({"null": null}),
            runner::Value::Boolean(b) => json!({"boolean": b}),
            runner::Value::Text(s) => json!({"text": s}),
            runner::Value::Signed(n) => json!({"signed": n}),
            runner::Value::Unsigned(n) => json!({"unsigned": n}),
            runner::Value::Float(n) => json!({"float": n}),
            runner::Value::Bytes(bytes) => json!({"bytes": bytes}),
            runner::Value::Path(p) => json!({"path": p.value.clone()}),
            runner::Value::Url(u) => json!({"url": u.value.clone()}),
            runner::Value::Datetime(d) => json!({"datetime": d.value.clone()}),
        }
    }

    fn selector_to_json(s: &runner::Selector) -> serde_json::Value {
        match s {
            runner::Selector::Field(f) => json!({"field": f}),
            runner::Selector::Index(i) => json!({"index": i}),
        }
    }

    fn output_ref_to_json(r: &runner::OutputRef) -> serde_json::Value {
        json!({
            "step_id": r.step_id.clone(),
            "path": r.path.iter().map(selector_to_json).collect::<Vec<_>>(),
        })
    }

    fn expr_to_json(e: &runner::Expr) -> serde_json::Value {
        match e {
            runner::Expr::Lit(v) => json!({"lit": value_to_json(v)}),
            runner::Expr::Get(r) => json!({"get": output_ref_to_json(r)}),
        }
    }

    fn op_to_json(op: &runner::Op) -> serde_json::Value {
        match op {
            runner::Op::DeclareService(d) => {
                let mut settings = d.settings.clone();
                settings.sort_by(|a, b| a.0.cmp(&b.0));
                json!({
                    "declare-service": {
                        "name": d.name.clone(),
                        "runtime": d.runtime.clone(),
                        "settings": settings
                            .iter()
                            .map(|(k, v)| json!([k, expr_to_json(v)]))
                            .collect::<Vec<_>>(),
                    }
                })
            }
            runner::Op::AllocatePort(p) => json!({"allocate-port": {"name": p.name.clone()}}),
            runner::Op::OciPull(p) => json!({"oci-pull": {"image": p.image.clone()}}),
            runner::Op::RenderTemplate(t) => {
                json!({"render-template": {"template": t.template.clone()}})
            }
            runner::Op::WriteFile(w) => json!({
                "write-file": {
                    "path": w.path.value.clone(),
                    "contents": w.contents.clone(),
                }
            }),
        }
    }

    let mut requested = plan.requested_capabilities.clone();
    requested.sort();

    let mut steps = plan.steps.clone();
    steps.sort_by(|a, b| a.id.cmp(&b.id));

    let steps_json = steps
        .iter()
        .map(|s| {
            let mut needs = s.needs.clone();
            needs.sort();
            json!({
                "id": s.id.clone(),
                "needs": needs,
                "op": op_to_json(&s.op),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "ir_version": plan.ir_version,
        "requested_capabilities": requested,
        "steps": steps_json,
    })
}

/// Convenience API for one-shot plugin apply.
pub fn apply_plugin(
    component_path: &Path,
    ctx: WorkspaceContext,
    caps: HostCapabilities,
    spec: ServiceSpec,
) -> Result<runner::PlanResult> {
    PluginRunner::new()?.apply(component_path, ctx, caps, spec)
}

/// Apply plugins to a configuration.
///
/// Discovers plugins from the project and user directories, runs detect/apply
/// for each service, and merges the resulting plans into the config.
///
/// # Arguments
/// * `config` - The loaded configuration to modify
/// * `project_root` - Path to the project root directory
/// * `allocator` - Port allocator for safe port assignment
///
/// # Returns
/// Ok with a vector of port guards that must be kept alive until services bind.
///
/// Plugin discovery and application failures are logged as warnings but do not
/// fail the overall operation - plugins are optional enhancements.
pub fn apply_plugins_to_config(
    config: &mut locald_core::config::LocaldConfig,
    project_root: &Path,
    allocator: &PortAllocator,
) -> Result<Vec<PortGuard>> {
    use tracing::{debug, info, warn};

    // Accumulate all port guards from plugin plans
    let mut all_guards = Vec::new();

    // Discover plugins
    let plugin_paths = discover_plugins(project_root);

    if plugin_paths.is_empty() {
        debug!("No plugins discovered");
        return Ok(all_guards);
    }

    info!("Discovered {} plugin(s)", plugin_paths.len());

    // Create plugin runner
    let runner = match PluginRunner::new() {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to create plugin runner: {}", e);
            return Ok(all_guards); // Continue without plugins
        }
    };

    // Get host capabilities
    let caps = default_capabilities();

    // Create workspace context
    let workspace_id = config
        .project
        .workspace
        .clone()
        .or_else(|| config.project.constellation.clone())
        .unwrap_or_else(|| config.project.name.clone());

    let ctx = WorkspaceContext {
        workspace_id,
        root: project_root.to_string_lossy().to_string(),
    };

    // Process each service through each plugin
    let service_names: Vec<String> = config.services.keys().cloned().collect();

    for service_name in &service_names {
        let service_config = &config.services[service_name];

        // Build ServiceSpec for this service
        let spec = ServiceSpec {
            name: service_name.clone(),
            kind: classify_service_kind(service_config),
            depends_on: service_config.depends_on().clone(),
            config: extract_service_config_kvs(service_config),
        };

        // Try each plugin
        for plugin_path in &plugin_paths {
            let plugin_name = plugin_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            // Detect if plugin applies
            let detects = match runner.detect(plugin_path, ctx.clone(), spec.clone()) {
                Ok(Some(reason)) => {
                    info!(
                        "Plugin '{}' applies to service '{}': {}",
                        plugin_name, service_name, reason
                    );
                    true
                }
                Ok(None) => {
                    debug!(
                        "Plugin '{}' does not apply to service '{}'",
                        plugin_name, service_name
                    );
                    false
                }
                Err(e) => {
                    warn!(
                        "Plugin '{}' detect failed for service '{}': {}",
                        plugin_name, service_name, e
                    );
                    false
                }
            };

            if !detects {
                continue;
            }

            // Apply plugin
            let plan_result =
                match runner.apply(plugin_path, ctx.clone(), caps.clone(), spec.clone()) {
                    Ok(result) => result,
                    Err(e) => {
                        warn!(
                            "Plugin '{}' apply failed for service '{}': {}",
                            plugin_name, service_name, e
                        );
                        continue;
                    }
                };

            // Process plan result
            let plan = match plan_result {
                Ok(plan) => plan,
                Err(diag) => {
                    warn!(
                        "Plugin '{}' returned diagnostics for service '{}': errors={:?}, warnings={:?}",
                        plugin_name, service_name, diag.errors, diag.warnings
                    );
                    continue;
                }
            };

            // Apply plan to config
            match apply_plan_to_config(config, &plan, &caps, allocator) {
                Ok((outputs, guards)) => {
                    info!(
                        "Plugin '{}' applied successfully to service '{}' ({} steps, {} outputs)",
                        plugin_name,
                        service_name,
                        plan.steps.len(),
                        outputs.len()
                    );
                    all_guards.extend(guards);
                }
                Err(diag) => {
                    warn!(
                        "Failed to apply plan from plugin '{}' for service '{}': {:?}",
                        plugin_name, service_name, diag
                    );
                }
            }
        }
    }

    Ok(all_guards)
}

/// Classify a service's runtime kind for plugin detection.
fn classify_service_kind(svc: &locald_core::config::ServiceConfig) -> String {
    use locald_core::config::{ServiceConfig, TypedServiceConfig};

    match svc {
        ServiceConfig::Typed(TypedServiceConfig::Exec(_)) | ServiceConfig::Legacy(_) => {
            "exec".to_string()
        }
        ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => "worker".to_string(),
        ServiceConfig::Typed(TypedServiceConfig::Container(_)) => "container".to_string(),
        ServiceConfig::Typed(TypedServiceConfig::Postgres(_)) => "postgres".to_string(),
        ServiceConfig::Typed(TypedServiceConfig::Site(_)) => "site".to_string(),
    }
}

/// Extract service config as key-value pairs for plugins.
fn extract_service_config_kvs(
    svc: &locald_core::config::ServiceConfig,
) -> Vec<(String, runner::Value)> {
    use locald_core::config::{ServiceConfig, TypedServiceConfig};

    let mut kvs = Vec::new();

    // Extract common config
    let env = svc.env();
    for (k, v) in env {
        kvs.push((format!("env.{k}"), runner::Value::Text(v.clone())));
    }

    if let Some(port) = svc.port() {
        kvs.push(("port".to_string(), runner::Value::Unsigned(u64::from(port))));
    }

    // Extract type-specific config
    match svc {
        ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
            if let Some(cmd) = &exec.command {
                kvs.push(("command".to_string(), runner::Value::Text(cmd.clone())));
            }
            if let Some(wd) = &exec.workdir {
                kvs.push(("workdir".to_string(), runner::Value::Text(wd.clone())));
            }
        }
        ServiceConfig::Typed(TypedServiceConfig::Worker(worker)) => {
            if !worker.command.is_empty() {
                kvs.push((
                    "command".to_string(),
                    runner::Value::Text(worker.command.clone()),
                ));
            }
            if let Some(wd) = &worker.workdir {
                kvs.push(("workdir".to_string(), runner::Value::Text(wd.clone())));
            }
        }
        ServiceConfig::Typed(TypedServiceConfig::Container(container)) => {
            if !container.image.is_empty() {
                kvs.push((
                    "image".to_string(),
                    runner::Value::Text(container.image.clone()),
                ));
            }
            if let Some(cmd) = &container.command {
                kvs.push(("command".to_string(), runner::Value::Text(cmd.clone())));
            }
        }
        ServiceConfig::Typed(TypedServiceConfig::Postgres(postgres)) => {
            if let Some(ver) = &postgres.version {
                kvs.push(("version".to_string(), runner::Value::Text(ver.clone())));
            }
        }
        ServiceConfig::Typed(TypedServiceConfig::Site(site)) => {
            if !site.path.is_empty() {
                kvs.push(("path".to_string(), runner::Value::Text(site.path.clone())));
            }
        }
        ServiceConfig::Legacy(exec) => {
            if let Some(cmd) = &exec.command {
                kvs.push(("command".to_string(), runner::Value::Text(cmd.clone())));
            }
            if let Some(wd) = &exec.workdir {
                kvs.push(("workdir".to_string(), runner::Value::Text(wd.clone())));
            }
        }
    }

    kvs
}

#[cfg(test)]
mod tests {
    use super::normalized_plan_debug_json;
    use super::{
        apply_plugins_to_config, classify_service_kind, extract_service_config_kvs, runner,
    };
    use crate::plugins::runner::{DeclareServiceOp, Expr, Op, Plan, Selector, Step, Value};
    use locald_core::config::{
        CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, PostgresServiceConfig,
        ServiceConfig, SiteServiceConfig, TypedServiceConfig, WorkerServiceConfig,
    };
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn normalized_plan_debug_json_sorts_steps_needs_and_capabilities() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec!["b".to_string(), "a".to_string()],
            steps: vec![
                Step {
                    id: "b".to_string(),
                    needs: vec!["c".to_string(), "a".to_string()],
                    op: Op::AllocatePort(crate::plugins::runner::AllocatePortOp {
                        name: "redis".to_string(),
                    }),
                },
                Step {
                    id: "a".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "redis".to_string(),
                        runtime: "container".to_string(),
                        settings: vec![
                            ("z".to_string(), Expr::Lit(Value::Text("last".to_string()))),
                            (
                                "a".to_string(),
                                Expr::Get(crate::plugins::runner::OutputRef {
                                    step_id: "b".to_string(),
                                    path: vec![Selector::Field("port".to_string())],
                                }),
                            ),
                        ],
                    }),
                },
                Step {
                    id: "c".to_string(),
                    needs: vec![],
                    op: Op::OciPull(crate::plugins::runner::OciPullOp {
                        image: "redis:7".to_string(),
                    }),
                },
            ],
        };

        let got = normalized_plan_debug_json(&plan);

        let expected = json!({
            "ir_version": 1,
            "requested_capabilities": ["a", "b"],
            "steps": [
                {
                    "id": "a",
                    "needs": [],
                    "op": {
                        "declare-service": {
                            "name": "redis",
                            "runtime": "container",
                            "settings": [
                                ["a", {"get": {"step_id": "b", "path": [{"field": "port"}]}}],
                                ["z", {"lit": {"text": "last"}}]
                            ]
                        }
                    }
                },
                {
                    "id": "b",
                    "needs": ["a", "c"],
                    "op": {"allocate-port": {"name": "redis"}}
                },
                {
                    "id": "c",
                    "needs": [],
                    "op": {"oci-pull": {"image": "redis:7"}}
                }
            ]
        });

        assert_eq!(got, expected);
    }

    #[test]
    fn classify_service_kind_exec() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
            common: CommonServiceConfig::default(),
            command: Some("npm start".to_string()),
            workdir: None,
            build: None,
        }));

        assert_eq!(classify_service_kind(&config), "exec");
    }

    #[test]
    fn classify_service_kind_worker() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
            common: CommonServiceConfig::default(),
            command: "rake jobs:work".to_string(),
            workdir: None,
        }));

        assert_eq!(classify_service_kind(&config), "worker");
    }

    #[test]
    fn classify_service_kind_container() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Container(ContainerServiceConfig {
            common: CommonServiceConfig::default(),
            image: "redis:7".to_string(),
            command: None,
            container_port: None,
            workdir: None,
        }));

        assert_eq!(classify_service_kind(&config), "container");
    }

    #[test]
    fn classify_service_kind_postgres() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Postgres(PostgresServiceConfig {
            common: CommonServiceConfig::default(),
            version: Some("15".to_string()),
        }));

        assert_eq!(classify_service_kind(&config), "postgres");
    }

    #[test]
    fn classify_service_kind_site() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Site(SiteServiceConfig {
            common: CommonServiceConfig::default(),
            path: "./dist".to_string(),
            build: "npm run build".to_string(),
            name: "docs".to_string(),
        }));

        assert_eq!(classify_service_kind(&config), "site");
    }

    #[test]
    fn classify_service_kind_legacy() {
        let config = ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig::default(),
            command: Some("rails server".to_string()),
            workdir: None,
            build: None,
        });

        assert_eq!(classify_service_kind(&config), "exec");
    }

    #[test]
    fn extract_service_config_kvs_env_vars() {
        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "development".to_string());
        env.insert("DEBUG".to_string(), "true".to_string());

        let config = ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
            common: CommonServiceConfig {
                env,
                ..CommonServiceConfig::default()
            },
            command: Some("npm start".to_string()),
            workdir: None,
            build: None,
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(kvs.iter().any(|(k, v)| k == "env.NODE_ENV"
            && matches!(v, runner::Value::Text(s) if s == "development")));
        assert!(
            kvs.iter().any(
                |(k, v)| k == "env.DEBUG" && matches!(v, runner::Value::Text(s) if s == "true")
            )
        );
    }

    #[test]
    fn extract_service_config_kvs_port() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
            common: CommonServiceConfig {
                port: Some(3000),
                ..CommonServiceConfig::default()
            },
            command: Some("npm start".to_string()),
            workdir: None,
            build: None,
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(
            kvs.iter()
                .any(|(k, v)| k == "port" && matches!(v, runner::Value::Unsigned(3000)))
        );
    }

    #[test]
    fn extract_service_config_kvs_exec_fields() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
            common: CommonServiceConfig::default(),
            command: Some("npm start".to_string()),
            workdir: Some("./app".to_string()),
            build: None,
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(kvs.iter().any(
            |(k, v)| k == "command" && matches!(v, runner::Value::Text(s) if s == "npm start")
        ));
        assert!(kvs.iter().any(|(k, v)| k == "workdir"
            && matches!(v, runner::Value::Text(s) if s == "./app")));
    }

    #[test]
    fn extract_service_config_kvs_worker_fields() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
            common: CommonServiceConfig::default(),
            command: "rake jobs:work".to_string(),
            workdir: Some("./backend".to_string()),
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(
            kvs.iter().any(|(k, v)| k == "command"
                && matches!(v, runner::Value::Text(s) if s == "rake jobs:work"))
        );
        assert!(kvs.iter().any(
            |(k, v)| k == "workdir" && matches!(v, runner::Value::Text(s) if s == "./backend")
        ));
    }

    #[test]
    fn extract_service_config_kvs_container_image() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Container(ContainerServiceConfig {
            common: CommonServiceConfig::default(),
            image: "redis:7".to_string(),
            command: Some("redis-server --appendonly yes".to_string()),
            container_port: None,
            workdir: None,
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(kvs.iter().any(|(k, v)| k == "image"
            && matches!(v, runner::Value::Text(s) if s == "redis:7")));
        assert!(kvs.iter().any(|(k, v)| k == "command"
            && matches!(v, runner::Value::Text(s) if s == "redis-server --appendonly yes")));
    }

    #[test]
    fn extract_service_config_kvs_postgres_version() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Postgres(PostgresServiceConfig {
            common: CommonServiceConfig::default(),
            version: Some("15".to_string()),
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(
            kvs.iter()
                .any(|(k, v)| k == "version" && matches!(v, runner::Value::Text(s) if s == "15"))
        );
    }

    #[test]
    fn extract_service_config_kvs_site_path() {
        let config = ServiceConfig::Typed(TypedServiceConfig::Site(SiteServiceConfig {
            common: CommonServiceConfig::default(),
            path: "./dist".to_string(),
            build: "npm run build".to_string(),
            name: "docs".to_string(),
        }));

        let kvs = extract_service_config_kvs(&config);

        assert!(
            kvs.iter()
                .any(|(k, v)| k == "path" && matches!(v, runner::Value::Text(s) if s == "./dist"))
        );
    }

    #[test]
    fn extract_service_config_kvs_legacy_fields() {
        let config = ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig::default(),
            command: Some("rails server".to_string()),
            workdir: Some("./api".to_string()),
            build: None,
        });

        let kvs = extract_service_config_kvs(&config);

        assert!(
            kvs.iter().any(|(k, v)| k == "command"
                && matches!(v, runner::Value::Text(s) if s == "rails server"))
        );
        assert!(kvs.iter().any(|(k, v)| k == "workdir"
            && matches!(v, runner::Value::Text(s) if s == "./api")));
    }

    #[test]
    fn apply_plugins_to_config_no_plugins() {
        use crate::port_allocator::PortAllocator;
        use locald_core::config::{LocaldConfig, ProjectConfig};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        let mut config = LocaldConfig {
            project: ProjectConfig {
                name: "test".to_string(),
                workspace: None,
                constellation: None,
                domain: None,
            },
            plugins: HashMap::new(),
            services: HashMap::new(),
        };

        let allocator = PortAllocator::new();
        let result = apply_plugins_to_config(&mut config, project_root, &allocator);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_plugins_to_config_unchanged_when_no_match() {
        use crate::port_allocator::PortAllocator;
        use locald_core::config::{LocaldConfig, ProjectConfig};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        let mut config = LocaldConfig {
            project: ProjectConfig {
                name: "test".to_string(),
                workspace: None,
                constellation: None,
                domain: None,
            },
            plugins: HashMap::new(),
            services: HashMap::new(),
        };

        config.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
                common: CommonServiceConfig::default(),
                command: Some("npm start".to_string()),
                workdir: None,
                build: None,
            })),
        );

        let original_len = config.services.len();

        let allocator = PortAllocator::new();
        let result = apply_plugins_to_config(&mut config, project_root, &allocator);
        assert!(result.is_ok());
        assert_eq!(config.services.len(), original_len);
    }
}
