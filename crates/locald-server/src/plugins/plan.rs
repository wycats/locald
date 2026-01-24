use crate::plugins::runner::{
    AllocatePortOp, DeclareServiceOp, Diagnostics, Expr, HostCapabilities, Op, OutputRef, Plan,
    Selector, Step, Value,
};
use locald_core::config::{
    CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, LocaldConfig,
    PostgresServiceConfig, ServiceConfig, SiteServiceConfig, TypedServiceConfig,
    WorkerServiceConfig,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Step outputs: `step_id` -> `field_name` -> value.
/// Accumulated during plan execution so later steps can reference earlier outputs.
pub type StepOutputs = BTreeMap<String, BTreeMap<String, Value>>;

#[derive(Debug, thiserror::Error)]
pub enum PlanApplyError {
    #[error("plan validation failed: {0:?}")]
    Diagnostics(Diagnostics),
}

fn diagnostics_error<S: Into<String>>(msg: S) -> Diagnostics {
    Diagnostics {
        warnings: Vec::new(),
        errors: vec![msg.into()],
    }
}

fn diagnostics_errors(errors: Vec<String>) -> Diagnostics {
    Diagnostics {
        warnings: Vec::new(),
        errors,
    }
}

/// Validate a plugin-provided plan against host capabilities and structural invariants.
///
/// Returns `Ok(())` if the plan is structurally valid. Otherwise returns plugin-style diagnostics.
pub fn validate_plan(plan: &Plan, caps: &HostCapabilities) -> std::result::Result<(), Diagnostics> {
    let mut errors = Vec::<String>::new();

    if !caps.supported_ir_versions.contains(&plan.ir_version) {
        errors.push(format!(
            "unsupported plan ir-version {} (host supports {:?})",
            plan.ir_version, caps.supported_ir_versions
        ));
    }

    let granted: HashSet<&str> = caps.granted.iter().map(String::as_str).collect();
    for cap in &plan.requested_capabilities {
        if !granted.contains(cap.as_str()) {
            errors.push(format!(
                "plan requests capability '{cap}' which is not granted"
            ));
        }
    }

    // Step id uniqueness + non-empty.
    let mut ids = HashSet::<&str>::new();
    for step in &plan.steps {
        if step.id.trim().is_empty() {
            errors.push("plan contains a step with an empty id".to_string());
            continue;
        }
        if !ids.insert(step.id.as_str()) {
            errors.push(format!("duplicate step id '{}'", step.id));
        }
    }

    // Needs references must exist.
    let id_set: HashSet<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &plan.steps {
        for need in &step.needs {
            if need.trim().is_empty() {
                errors.push(format!("step '{}' has an empty dependency id", step.id));
            } else if !id_set.contains(need.as_str()) {
                errors.push(format!(
                    "step '{}' depends on unknown step '{}'",
                    step.id, need
                ));
            }
        }
    }

    // Detect cycles (Kahn). Only if basic invariants passed.
    if errors.is_empty() {
        if let Err(e) = validate_acyclic(plan) {
            errors.push(e);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(diagnostics_errors(errors))
    }
}

fn validate_acyclic(plan: &Plan) -> Result<(), String> {
    topo_order(plan).map(|_| ())
}

/// Apply a validated plan into a mutable `LocaldConfig`.
///
/// For Phase 29.1.3, we support:
/// - `declare-service`: adds a new service to the config (fails if it already exists)
/// - `allocate-port`: allocates a port and produces output `{ "port": <u16> }`
///
/// Other ops are rejected.
///
/// Returns accumulated step outputs for use in expression resolution.
pub fn apply_plan_to_config(
    config: &mut LocaldConfig,
    plan: &Plan,
    caps: &HostCapabilities,
) -> std::result::Result<StepOutputs, Diagnostics> {
    validate_plan(plan, caps)?;

    // Track outputs from executed steps.
    let mut outputs = StepOutputs::new();

    // Apply steps in topological order.
    let order = topo_order(plan).map_err(diagnostics_error)?;

    for step_id in order {
        let step = plan
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .ok_or_else(|| diagnostics_error("internal error: missing step during apply"))?;

        let step_output = apply_step(config, step, &outputs)?;
        if !step_output.is_empty() {
            outputs.insert(step.id.clone(), step_output);
        }
    }

    Ok(outputs)
}

fn topo_order(plan: &Plan) -> Result<Vec<String>, String> {
    // Deterministic Kahn topological sort.
    // - Lexicographic tie-break for ready nodes.
    // - Deterministic successor traversal order.
    let mut indegree: BTreeMap<String, usize> = BTreeMap::new();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for step in &plan.steps {
        indegree.insert(step.id.clone(), 0);
        edges.insert(step.id.clone(), Vec::new());
    }

    for step in &plan.steps {
        for need in &step.needs {
            let to = step.id.clone();
            let from = need.clone();
            edges.get_mut(&from).unwrap().push(to);
            *indegree.get_mut(&step.id).unwrap() += 1;
        }
    }

    for succs in edges.values_mut() {
        succs.sort();
    }

    let mut ready: BTreeSet<String> = BTreeSet::new();
    for (id, deg) in &indegree {
        if *deg == 0 {
            ready.insert(id.clone());
        }
    }

    let mut out = Vec::with_capacity(plan.steps.len());
    while let Some(id) = ready.iter().next().cloned() {
        ready.remove(&id);
        out.push(id.clone());

        for succ in edges.get(&id).unwrap() {
            let d = indegree.get_mut(succ).unwrap();
            *d -= 1;
            if *d == 0 {
                ready.insert(succ.clone());
            }
        }
    }

    if out.len() == plan.steps.len() {
        Ok(out)
    } else {
        Err("plan step graph contains a cycle".to_string())
    }
}

fn apply_step(
    config: &mut LocaldConfig,
    step: &Step,
    outputs: &StepOutputs,
) -> std::result::Result<BTreeMap<String, Value>, Diagnostics> {
    match &step.op {
        Op::DeclareService(op) => {
            apply_declare_service(config, op, outputs)?;
            Ok(BTreeMap::new()) // No outputs from declare-service
        }
        Op::AllocatePort(op) => allocate_port_for_service(config, op),
        Op::OciPull(_) => Err(diagnostics_error(
            "unsupported op 'oci-pull' (not implemented in Phase 29.1.3)".to_string(),
        )),
        Op::RenderTemplate(_) => Err(diagnostics_error(
            "unsupported op 'render-template' (not implemented in Phase 29.1.3)".to_string(),
        )),
        Op::WriteFile(_) => Err(diagnostics_error(
            "unsupported op 'write-file' (not implemented in Phase 29.1.3)".to_string(),
        )),
    }
}

/// Allocate a port for a service.
///
/// Returns outputs: `{ "port": Value::Unsigned(<allocated_port>) }`
fn allocate_port_for_service(
    config: &LocaldConfig,
    op: &AllocatePortOp,
) -> std::result::Result<BTreeMap<String, Value>, Diagnostics> {
    // Verify service exists
    if !config.services.contains_key(&op.name) {
        return Err(diagnostics_error(format!(
            "allocate-port references unknown service '{}'",
            op.name
        )));
    }

    // Allocate a free port by binding to port 0 and letting the OS choose.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| {
        diagnostics_error(format!("failed to allocate port for '{}': {}", op.name, e))
    })?;

    let port = listener
        .local_addr()
        .map_err(|e| diagnostics_error(format!("failed to get local address: {}", e)))?
        .port();

    drop(listener); // Release the port for the service to use

    // Return port as output
    let mut output = BTreeMap::new();
    output.insert("port".to_string(), Value::Unsigned(u64::from(port)));
    Ok(output)
}

/// Classify a service by its runtime type.
fn classify_service_type(svc: &ServiceConfig) -> &str {
    match svc {
        ServiceConfig::Typed(TypedServiceConfig::Exec(_)) | ServiceConfig::Legacy(_) => "exec",
        ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => "worker",
        ServiceConfig::Typed(TypedServiceConfig::Container(_)) => "container",
        ServiceConfig::Typed(TypedServiceConfig::Postgres(_)) => "postgres",
        ServiceConfig::Typed(TypedServiceConfig::Site(_)) => "site",
    }
}

/// Merge common service config fields with "User Wins" priority.
fn merge_common(existing: &mut CommonServiceConfig, plugin: &CommonServiceConfig) {
    // Env: plugin provides base, user overrides on conflict
    for (key, value) in &plugin.env {
        existing
            .env
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }

    // Port: user wins if set
    if existing.port.is_none() {
        existing.port = plugin.port;
    }

    // depends_on: union of both sets
    let mut deps_set: BTreeSet<String> = existing.depends_on.iter().cloned().collect();
    for dep in &plugin.depends_on {
        deps_set.insert(dep.clone());
    }
    existing.depends_on = deps_set.into_iter().collect();

    // health_check: user wins if set
    if existing.health_check.is_none() {
        existing.health_check.clone_from(&plugin.health_check);
    }

    // stop_signal: user wins if set
    if existing.stop_signal.is_none() {
        existing.stop_signal.clone_from(&plugin.stop_signal);
    }
}

/// Merge a plugin-generated service config into an existing user-defined service.
///
/// Merge strategy ("User Wins" priority):
/// - **env**: Plugin provides base, user overrides on key conflict
/// - **port**: User wins if set, else plugin value
/// - **`depends_on`**: Union of both sets
/// - **`health_check`**: User wins if set, else plugin value
/// - **runtime type**: Must match (error if types mismatch)
/// - **command/workdir/image**: User wins if set, else plugin value
fn merge_plugin_service_into_config(
    existing: &mut ServiceConfig,
    op: &DeclareServiceOp,
    outputs: &StepOutputs,
) -> std::result::Result<(), Diagnostics> {
    // Build the plugin service config as if declaring new
    let plugin_svc = build_service_from_op(op, outputs)?;

    // Check runtime type compatibility
    let existing_type = classify_service_type(existing);
    let plugin_type = classify_service_type(&plugin_svc);

    if existing_type != plugin_type {
        return Err(diagnostics_error(format!(
            "service '{}' type conflict: user defined as '{}', plugin declares as '{}'",
            op.name, existing_type, plugin_type
        )));
    }

    // Merge based on service type
    match (existing, plugin_svc) {
        (
            ServiceConfig::Typed(TypedServiceConfig::Exec(existing_exec)),
            ServiceConfig::Typed(TypedServiceConfig::Exec(plugin_exec)),
        ) => {
            merge_common(&mut existing_exec.common, &plugin_exec.common);
            if existing_exec.command.is_none() {
                existing_exec.command = plugin_exec.command;
            }
            if existing_exec.workdir.is_none() {
                existing_exec.workdir = plugin_exec.workdir;
            }
        }
        (
            ServiceConfig::Typed(TypedServiceConfig::Worker(existing_worker)),
            ServiceConfig::Typed(TypedServiceConfig::Worker(plugin_worker)),
        ) => {
            merge_common(&mut existing_worker.common, &plugin_worker.common);
            if existing_worker.command.is_empty() {
                existing_worker.command = plugin_worker.command;
            }
            if existing_worker.workdir.is_none() {
                existing_worker.workdir = plugin_worker.workdir;
            }
        }
        (
            ServiceConfig::Typed(TypedServiceConfig::Container(existing_container)),
            ServiceConfig::Typed(TypedServiceConfig::Container(plugin_container)),
        ) => {
            merge_common(&mut existing_container.common, &plugin_container.common);
            if existing_container.image.is_empty() {
                existing_container.image = plugin_container.image;
            }
            if existing_container.command.is_none() {
                existing_container.command = plugin_container.command;
            }
            if existing_container.workdir.is_none() {
                existing_container.workdir = plugin_container.workdir;
            }
            if existing_container.container_port.is_none() {
                existing_container.container_port = plugin_container.container_port;
            }
        }
        (
            ServiceConfig::Typed(TypedServiceConfig::Postgres(existing_postgres)),
            ServiceConfig::Typed(TypedServiceConfig::Postgres(plugin_postgres)),
        ) => {
            merge_common(&mut existing_postgres.common, &plugin_postgres.common);
            if existing_postgres.version.is_none() {
                existing_postgres.version = plugin_postgres.version;
            }
        }
        (
            ServiceConfig::Typed(TypedServiceConfig::Site(existing_site)),
            ServiceConfig::Typed(TypedServiceConfig::Site(plugin_site)),
        ) => {
            merge_common(&mut existing_site.common, &plugin_site.common);
            if existing_site.path.is_empty() {
                existing_site.path = plugin_site.path;
            }
            if existing_site.build.is_empty() {
                existing_site.build = plugin_site.build;
            }
        }
        (ServiceConfig::Legacy(existing_legacy), ServiceConfig::Legacy(plugin_legacy)) => {
            merge_common(&mut existing_legacy.common, &plugin_legacy.common);
            if existing_legacy.command.is_none() {
                existing_legacy.command = plugin_legacy.command;
            }
            if existing_legacy.workdir.is_none() {
                existing_legacy.workdir = plugin_legacy.workdir;
            }
        }
        _ => {
            // This shouldn't happen if classify_service_type is correct
            return Err(diagnostics_error(format!(
                "internal error: service type mismatch during merge for '{}'",
                op.name
            )));
        }
    }

    Ok(())
}

/// Build a service config from a [`DeclareServiceOp`].
///
/// This is extracted from [`apply_declare_service`] to support merging.
fn build_service_from_op(
    op: &DeclareServiceOp,
    outputs: &StepOutputs,
) -> std::result::Result<ServiceConfig, Diagnostics> {
    let mut common = CommonServiceConfig::default();

    let mut exec = ExecServiceConfig::default();
    let mut worker = WorkerServiceConfig::default();
    let mut container = ContainerServiceConfig::default();
    let mut postgres = PostgresServiceConfig::default();
    let mut site = SiteServiceConfig::default();

    // Keep common in sync.
    exec.common = common.clone();
    worker.common = common.clone();
    container.common = common.clone();
    postgres.common = common.clone();
    site.common = common.clone();

    // Parse settings.
    for (key, expr) in &op.settings {
        let v = eval_expr(expr, outputs).map_err(|e| {
            diagnostics_error(format!(
                "failed to evaluate expression for setting '{key}': {e}"
            ))
        })?;

        // Common settings.
        if let Some(env_key) = key.strip_prefix("env.") {
            let env_key = env_key.trim();
            if env_key.is_empty() {
                return Err(diagnostics_error("env.* setting has empty key"));
            }
            // Allow text or unsigned (for port references)
            let text = value_to_string(&v).ok_or_else(|| {
                diagnostics_error(format!("env.{env_key} must be a text or unsigned integer"))
            })?;
            common.env.insert(env_key.to_string(), text);
            continue;
        }

        match key.as_str() {
            "port" => {
                let port = as_u16(&v).ok_or_else(|| {
                    diagnostics_error("port must be an unsigned integer <= 65535")
                })?;
                common.port = Some(port);
            }
            "command" => {
                let cmd = as_text(&v)
                    .ok_or_else(|| diagnostics_error("command must be a text literal"))?;
                exec.command = Some(cmd.to_string());
                worker.command = cmd.to_string();
                container.command = Some(cmd.to_string());
            }
            "workdir" => {
                let wd = as_text(&v)
                    .ok_or_else(|| diagnostics_error("workdir must be a text literal"))?;
                exec.workdir = Some(wd.to_string());
                worker.workdir = Some(wd.to_string());
                container.workdir = Some(wd.to_string());
            }
            "image" => {
                let image =
                    as_text(&v).ok_or_else(|| diagnostics_error("image must be a text literal"))?;
                container.image = image.to_string();
            }
            "container_port" => {
                let port = as_u16(&v).ok_or_else(|| {
                    diagnostics_error("container_port must be an unsigned integer <= 65535")
                })?;
                container.container_port = Some(port);
            }
            "postgres.version" | "version" => {
                let ver = as_text(&v)
                    .ok_or_else(|| diagnostics_error("postgres version must be a text literal"))?;
                postgres.version = Some(ver.to_string());
            }
            "site.path" | "path" => {
                let p = as_text(&v)
                    .ok_or_else(|| diagnostics_error("site path must be a text literal"))?;
                site.path = p.to_string();
            }
            "site.build" | "build" => {
                let b = as_text(&v)
                    .ok_or_else(|| diagnostics_error("site build must be a text literal"))?;
                site.build = b.to_string();
            }
            other => {
                return Err(diagnostics_error(format!(
                    "unsupported declare-service setting '{other}'"
                )));
            }
        }
    }

    // Copy accumulated common back into each typed config.
    exec.common = common.clone();
    worker.common = common.clone();
    container.common = common.clone();
    postgres.common = common.clone();
    site.common = common;

    let runtime = op.runtime.trim().to_lowercase();
    let svc = match runtime.as_str() {
        "exec" => {
            if exec.command.is_none() {
                return Err(diagnostics_error(
                    "declare-service runtime 'exec' requires a 'command' setting".to_string(),
                ));
            }
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec))
        }
        "worker" => {
            if worker.command.trim().is_empty() {
                return Err(diagnostics_error(
                    "declare-service runtime 'worker' requires a 'command' setting".to_string(),
                ));
            }
            ServiceConfig::Typed(TypedServiceConfig::Worker(worker))
        }
        "container" => {
            if container.image.trim().is_empty() {
                return Err(diagnostics_error(
                    "declare-service runtime 'container' requires an 'image' setting".to_string(),
                ));
            }
            ServiceConfig::Typed(TypedServiceConfig::Container(container))
        }
        "postgres" => ServiceConfig::Typed(TypedServiceConfig::Postgres(postgres)),
        "site" => {
            if site.path.trim().is_empty() {
                return Err(diagnostics_error(
                    "declare-service runtime 'site' requires a 'path' setting".to_string(),
                ));
            }
            ServiceConfig::Typed(TypedServiceConfig::Site(site))
        }
        other => {
            return Err(diagnostics_error(format!(
                "unsupported declare-service runtime '{other}'"
            )));
        }
    };

    Ok(svc)
}

fn apply_declare_service(
    config: &mut LocaldConfig,
    op: &DeclareServiceOp,
    outputs: &StepOutputs,
) -> std::result::Result<(), Diagnostics> {
    let name = op.name.trim();
    if name.is_empty() {
        return Err(diagnostics_error("declare-service has an empty name"));
    }

    // If service already exists, merge instead of error
    if let Some(existing) = config.services.get_mut(name) {
        return merge_plugin_service_into_config(existing, op, outputs);
    }

    // Otherwise create new service
    let svc = build_service_from_op(op, outputs)?;
    config.services.insert(name.to_string(), svc);
    Ok(())
}

/// Evaluate an expression, resolving references to step outputs.
fn eval_expr(expr: &Expr, outputs: &StepOutputs) -> Result<Value, String> {
    match expr {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Get(output_ref) => resolve_output_ref(output_ref, outputs),
    }
}

/// Resolve an output reference by looking up the step output and traversing the path.
fn resolve_output_ref(output_ref: &OutputRef, outputs: &StepOutputs) -> Result<Value, String> {
    // Step 1: Find the step's output map
    let step_output = outputs
        .get(&output_ref.step_id)
        .ok_or_else(|| format!("reference to unknown step '{}'", output_ref.step_id))?;

    // Step 2: Validate path is not empty
    if output_ref.path.is_empty() {
        return Err("output reference must have at least one selector".to_string());
    }

    // Step 3: Traverse the path
    // For Phase 29, we only support a single field selector on flat maps
    let mut current_value: Option<&Value> = None;

    for (idx, selector) in output_ref.path.iter().enumerate() {
        match selector {
            Selector::Field(field_name) => {
                if idx == 0 {
                    // First selector: look up in step output map
                    current_value = Some(step_output.get(field_name).ok_or_else(|| {
                        format!(
                            "step '{}' has no output field '{}'",
                            output_ref.step_id, field_name
                        )
                    })?);
                } else {
                    // Subsequent selectors: would need nested object support
                    // For Phase 29, we only have flat maps
                    return Err(format!(
                        "nested field access not supported (step '{}', path index {})",
                        output_ref.step_id, idx
                    ));
                }
            }
            Selector::Index(_) => {
                // Arrays not yet in Value enum
                return Err(format!(
                    "index selector not supported (step '{}', path index {})",
                    output_ref.step_id, idx
                ));
            }
        }
    }

    Ok(current_value.unwrap().clone())
}

fn as_text(v: &Value) -> Option<&str> {
    match v {
        Value::Text(s) => Some(s.as_str()),
        Value::Null
        | Value::Boolean(_)
        | Value::Signed(_)
        | Value::Unsigned(_)
        | Value::Float(_)
        | Value::Bytes(_)
        | Value::Path(_)
        | Value::Url(_)
        | Value::Datetime(_) => None,
    }
}

fn as_u16(v: &Value) -> Option<u16> {
    match v {
        Value::Unsigned(n) => u16::try_from(*n).ok(),
        Value::Signed(n) => u16::try_from(*n).ok(),
        Value::Null
        | Value::Boolean(_)
        | Value::Text(_)
        | Value::Float(_)
        | Value::Bytes(_)
        | Value::Path(_)
        | Value::Url(_)
        | Value::Datetime(_) => None,
    }
}

/// Convert a Value to a string representation (for env vars).
/// Supports text (as-is) and unsigned integers (converted to decimal).
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Unsigned(n) => Some(n.to_string()),
        Value::Signed(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Null
        | Value::Float(_)
        | Value::Bytes(_)
        | Value::Path(_)
        | Value::Url(_)
        | Value::Datetime(_) => None,
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::plugins::runner::{AllocatePortOp, OutputRef, Selector};

    fn caps() -> HostCapabilities {
        HostCapabilities {
            supported_ir_versions: vec![1],
            granted: vec![],
        }
    }

    fn base_config() -> LocaldConfig {
        LocaldConfig::default()
    }

    #[test]
    fn rejects_unknown_dependency() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "b".to_string(),
                needs: vec!["a".to_string()],
                op: Op::AllocatePort(AllocatePortOp {
                    name: "web".to_string(),
                }),
            }],
        };

        let err = validate_plan(&plan, &caps()).unwrap_err();
        assert!(err.errors.iter().any(|e| e.contains("unknown step")));
    }

    #[test]
    fn applies_declare_service_exec() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![
                        (
                            "command".to_string(),
                            Expr::Lit(Value::Text("npm start".to_string())),
                        ),
                        ("port".to_string(), Expr::Lit(Value::Unsigned(3000))),
                        (
                            "env.NODE_ENV".to_string(),
                            Expr::Lit(Value::Text("development".to_string())),
                        ),
                    ],
                }),
            }],
        };

        let mut cfg = base_config();
        let outputs = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        // declare-service produces no outputs
        assert!(outputs.is_empty());

        let svc = cfg.services.get("web").expect("service inserted");
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                assert_eq!(exec.command.as_deref(), Some("npm start"));
                assert_eq!(exec.common.port, Some(3000));
                assert_eq!(
                    exec.common.env.get("NODE_ENV").map(String::as_str),
                    Some("development")
                );
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => {
                panic!("unexpected service config type")
            }
        }
    }

    #[test]
    fn allocate_port_produces_output() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "declare".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "web".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("npm start".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "allocate".to_string(),
                    needs: vec!["declare".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "web".to_string(),
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        let outputs = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        // Verify port was allocated
        assert!(outputs.contains_key("allocate"));
        let allocate_output = outputs.get("allocate").unwrap();
        assert!(allocate_output.contains_key("port"));

        // Verify it's a valid port number
        match allocate_output.get("port").unwrap() {
            Value::Unsigned(port) => {
                assert!(*port > 1024, "Should be an ephemeral port");
                assert!(*port < 65536, "Should be a valid port");
            }
            Value::Null
            | Value::Boolean(_)
            | Value::Text(_)
            | Value::Signed(_)
            | Value::Float(_)
            | Value::Bytes(_)
            | Value::Path(_)
            | Value::Url(_)
            | Value::Datetime(_) => {
                panic!("Port output should be an unsigned integer");
            }
        }
    }

    #[test]
    fn step_outputs_tracking_multiple_ports() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "s1".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "svc1".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo hi".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "s2".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "svc2".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo bye".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "port1".to_string(),
                    needs: vec!["s1".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "svc1".to_string(),
                    }),
                },
                Step {
                    id: "port2".to_string(),
                    needs: vec!["s2".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "svc2".to_string(),
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        let outputs = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        // Should have outputs for both port allocations
        assert_eq!(outputs.len(), 2);
        assert!(outputs.contains_key("port1"));
        assert!(outputs.contains_key("port2"));

        // Both should have different ports
        let port1 = match outputs.get("port1").unwrap().get("port").unwrap() {
            Value::Unsigned(p) => *p,
            Value::Null
            | Value::Boolean(_)
            | Value::Text(_)
            | Value::Signed(_)
            | Value::Float(_)
            | Value::Bytes(_)
            | Value::Path(_)
            | Value::Url(_)
            | Value::Datetime(_) => panic!("Expected unsigned port for port1"),
        };
        let port2 = match outputs.get("port2").unwrap().get("port").unwrap() {
            Value::Unsigned(p) => *p,
            Value::Null
            | Value::Boolean(_)
            | Value::Text(_)
            | Value::Signed(_)
            | Value::Float(_)
            | Value::Bytes(_)
            | Value::Path(_)
            | Value::Url(_)
            | Value::Datetime(_) => panic!("Expected unsigned port for port2"),
        };

        assert_ne!(port1, port2, "Ports should be different");
    }

    #[test]
    fn allocate_port_rejects_unknown_service() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "allocate".to_string(),
                needs: vec![],
                op: Op::AllocatePort(AllocatePortOp {
                    name: "nonexistent".to_string(),
                }),
            }],
        };

        let mut cfg = base_config();
        let err = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap_err();
        assert!(err.errors.iter().any(|e| e.contains("unknown service")));
    }

    // $ref resolution tests

    #[test]
    fn resolves_port_reference_in_env_var() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "declare_db".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "postgres".to_string(),
                        runtime: "postgres".to_string(),
                        settings: vec![],
                    }),
                },
                Step {
                    id: "alloc_port".to_string(),
                    needs: vec!["declare_db".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "postgres".to_string(),
                    }),
                },
                Step {
                    id: "declare_app".to_string(),
                    needs: vec!["alloc_port".to_string()],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "app".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![
                            (
                                "command".to_string(),
                                Expr::Lit(Value::Text("npm start".to_string())),
                            ),
                            (
                                "env.DATABASE_PORT".to_string(),
                                Expr::Get(OutputRef {
                                    step_id: "alloc_port".to_string(),
                                    path: vec![Selector::Field("port".to_string())],
                                }),
                            ),
                        ],
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        let outputs = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let app_svc = cfg.services.get("app").expect("app service created");
        match app_svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                let db_port_env = exec
                    .common
                    .env
                    .get("DATABASE_PORT")
                    .expect("DATABASE_PORT env var set");

                // Should be the port allocated for postgres (as string)
                let allocated_port = outputs
                    .get("alloc_port")
                    .and_then(|o| o.get("port"))
                    .and_then(|v| match v {
                        Value::Unsigned(p) => Some(*p),
                        _ => None,
                    })
                    .expect("port should be allocated");

                assert_eq!(db_port_env, &allocated_port.to_string());
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => {
                panic!("expected exec service")
            }
        }
    }

    #[test]
    fn rejects_reference_to_missing_step() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "declare_app".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "app".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![
                        (
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo".to_string())),
                        ),
                        (
                            "port".to_string(),
                            Expr::Get(OutputRef {
                                step_id: "nonexistent".to_string(),
                                path: vec![Selector::Field("port".to_string())],
                            }),
                        ),
                    ],
                }),
            }],
        };

        let mut cfg = base_config();
        let err = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap_err();
        assert!(err.errors.iter().any(|e| e.contains("unknown step")));
    }

    #[test]
    fn rejects_reference_to_missing_field() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "svc".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "svc".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "alloc_port".to_string(),
                    needs: vec!["svc".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "svc".to_string(),
                    }),
                },
                Step {
                    id: "use_it".to_string(),
                    needs: vec!["alloc_port".to_string()],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "app".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![
                            (
                                "command".to_string(),
                                Expr::Lit(Value::Text("echo".to_string())),
                            ),
                            (
                                "port".to_string(),
                                Expr::Get(OutputRef {
                                    step_id: "alloc_port".to_string(),
                                    path: vec![Selector::Field("nonexistent".to_string())],
                                }),
                            ),
                        ],
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        let err = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap_err();
        assert!(err.errors.iter().any(|e| e.contains("no output field")));
    }

    #[test]
    fn handles_multiple_references_in_one_service() {
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "declare_db".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "db".to_string(),
                        runtime: "postgres".to_string(),
                        settings: vec![],
                    }),
                },
                Step {
                    id: "declare_redis".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "redis".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("redis-server".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "port_db".to_string(),
                    needs: vec!["declare_db".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "db".to_string(),
                    }),
                },
                Step {
                    id: "port_redis".to_string(),
                    needs: vec!["declare_redis".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "redis".to_string(),
                    }),
                },
                Step {
                    id: "declare_app".to_string(),
                    needs: vec!["port_db".to_string(), "port_redis".to_string()],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "app".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![
                            (
                                "command".to_string(),
                                Expr::Lit(Value::Text("node app.js".to_string())),
                            ),
                            (
                                "env.DB_PORT".to_string(),
                                Expr::Get(OutputRef {
                                    step_id: "port_db".to_string(),
                                    path: vec![Selector::Field("port".to_string())],
                                }),
                            ),
                            (
                                "env.REDIS_PORT".to_string(),
                                Expr::Get(OutputRef {
                                    step_id: "port_redis".to_string(),
                                    path: vec![Selector::Field("port".to_string())],
                                }),
                            ),
                        ],
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let app = cfg.services.get("app").expect("app service");
        match app {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                assert!(exec.common.env.contains_key("DB_PORT"));
                assert!(exec.common.env.contains_key("REDIS_PORT"));

                // Verify they're different ports
                let db_port = &exec.common.env["DB_PORT"];
                let redis_port = &exec.common.env["REDIS_PORT"];
                assert_ne!(db_port, redis_port, "Ports should be different");
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec"),
        }
    }

    #[test]
    fn uses_port_reference_for_service_port() {
        // Port is stored as Value::Unsigned and should work for the port setting
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![
                Step {
                    id: "svc".to_string(),
                    needs: vec![],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "svc".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![(
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo".to_string())),
                        )],
                    }),
                },
                Step {
                    id: "port".to_string(),
                    needs: vec!["svc".to_string()],
                    op: Op::AllocatePort(AllocatePortOp {
                        name: "svc".to_string(),
                    }),
                },
                Step {
                    id: "app".to_string(),
                    needs: vec!["port".to_string()],
                    op: Op::DeclareService(DeclareServiceOp {
                        name: "app".to_string(),
                        runtime: "exec".to_string(),
                        settings: vec![
                            (
                                "command".to_string(),
                                Expr::Lit(Value::Text("node app.js".to_string())),
                            ),
                            (
                                "port".to_string(),
                                Expr::Get(OutputRef {
                                    step_id: "port".to_string(),
                                    path: vec![Selector::Field("port".to_string())],
                                }),
                            ),
                        ],
                    }),
                },
            ],
        };

        let mut cfg = base_config();
        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let app = cfg.services.get("app").expect("app service");
        match app {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                assert!(exec.common.port.is_some());
                let port = exec.common.port.unwrap();
                assert!(port > 1024, "Should be an ephemeral port");
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec"),
        }
    }

    #[test]
    fn merges_plugin_env_into_existing_service() {
        // User defines a service with some env vars
        let mut cfg = base_config();
        let mut exec = ExecServiceConfig::default();
        exec.command = Some("npm start".to_string());
        exec.common
            .env
            .insert("USER_VAR".to_string(), "user_value".to_string());
        cfg.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)),
        );

        // Plugin tries to add the same service with different env vars
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![
                        (
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo plugin".to_string())),
                        ),
                        (
                            "env.PLUGIN_VAR".to_string(),
                            Expr::Lit(Value::Text("plugin_value".to_string())),
                        ),
                        (
                            "env.USER_VAR".to_string(),
                            Expr::Lit(Value::Text("should_be_ignored".to_string())),
                        ),
                    ],
                }),
            }],
        };

        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let svc = cfg.services.get("web").unwrap();
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                // User's env var wins on conflict
                assert_eq!(exec.common.env.get("USER_VAR").unwrap(), "user_value");
                // Plugin's unique env var is added
                assert_eq!(exec.common.env.get("PLUGIN_VAR").unwrap(), "plugin_value");
                // User's command wins
                assert_eq!(exec.command.as_deref(), Some("npm start"));
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec service"),
        }
    }

    #[test]
    fn user_port_wins_over_plugin() {
        let mut cfg = base_config();
        let mut exec = ExecServiceConfig::default();
        exec.command = Some("npm start".to_string());
        exec.common.port = Some(4000);
        cfg.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)),
        );

        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![
                        (
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo".to_string())),
                        ),
                        ("port".to_string(), Expr::Lit(Value::Unsigned(3000))),
                    ],
                }),
            }],
        };

        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let svc = cfg.services.get("web").unwrap();
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                // User's port wins
                assert_eq!(exec.common.port, Some(4000));
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec service"),
        }
    }

    #[test]
    fn unions_dependencies() {
        let mut cfg = base_config();
        let mut exec = ExecServiceConfig::default();
        exec.command = Some("npm start".to_string());
        exec.common.depends_on = vec!["db".to_string()];
        cfg.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)),
        );

        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![(
                        "command".to_string(),
                        Expr::Lit(Value::Text("echo".to_string())),
                    )],
                }),
            }],
        };

        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let svc = cfg.services.get("web").unwrap();
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                // User's dependency is preserved
                assert!(exec.common.depends_on.contains(&"db".to_string()));
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec service"),
        }
    }

    #[test]
    fn rejects_runtime_type_mismatch() {
        // User defines an exec service
        let mut cfg = base_config();
        let mut exec = ExecServiceConfig::default();
        exec.command = Some("npm start".to_string());
        cfg.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)),
        );

        // Plugin tries to declare it as postgres
        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "postgres".to_string(),
                    settings: vec![],
                }),
            }],
        };

        let err = apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap_err();
        assert!(err.errors.iter().any(|e| e.contains("type conflict")));
    }

    #[test]
    fn user_env_wins_on_conflict() {
        let mut cfg = base_config();
        let mut exec = ExecServiceConfig::default();
        exec.command = Some("npm start".to_string());
        exec.common
            .env
            .insert("KEY".to_string(), "user_value".to_string());
        cfg.services.insert(
            "web".to_string(),
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)),
        );

        let plan = Plan {
            ir_version: 1,
            requested_capabilities: vec![],
            steps: vec![Step {
                id: "s1".to_string(),
                needs: vec![],
                op: Op::DeclareService(DeclareServiceOp {
                    name: "web".to_string(),
                    runtime: "exec".to_string(),
                    settings: vec![
                        (
                            "command".to_string(),
                            Expr::Lit(Value::Text("echo".to_string())),
                        ),
                        (
                            "env.KEY".to_string(),
                            Expr::Lit(Value::Text("plugin_value".to_string())),
                        ),
                    ],
                }),
            }],
        };

        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let svc = cfg.services.get("web").unwrap();
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                // User's value should win
                assert_eq!(exec.common.env.get("KEY").unwrap(), "user_value");
            }
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => panic!("expected exec service"),
        }
    }
}
