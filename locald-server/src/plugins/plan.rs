use crate::plugins::runner::{
    DeclareServiceOp, Diagnostics, Expr, HostCapabilities, Op, Plan, Step, Value,
};
use locald_core::config::{
    CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, LocaldConfig,
    PostgresServiceConfig, ServiceConfig, SiteServiceConfig, TypedServiceConfig,
    WorkerServiceConfig,
};
use std::collections::{HashMap, HashSet, VecDeque};

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

    let granted: HashSet<&str> = caps.granted.iter().map(|s| s.as_str()).collect();
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
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();

    for step in &plan.steps {
        indegree.insert(step.id.as_str(), 0);
        edges.insert(step.id.as_str(), Vec::new());
    }

    for step in &plan.steps {
        for need in &step.needs {
            let to = step.id.as_str();
            let from = need.as_str();
            edges.get_mut(from).unwrap().push(to);
            *indegree.get_mut(to).unwrap() += 1;
        }
    }

    let mut q = VecDeque::new();
    for (id, deg) in &indegree {
        if *deg == 0 {
            q.push_back(*id);
        }
    }

    let mut seen = 0usize;
    while let Some(id) = q.pop_front() {
        seen += 1;
        for succ in edges.get(id).unwrap() {
            let d = indegree.get_mut(succ).unwrap();
            *d -= 1;
            if *d == 0 {
                q.push_back(*succ);
            }
        }
    }

    if seen == indegree.len() {
        Ok(())
    } else {
        Err("plan step graph contains a cycle".to_string())
    }
}

/// Apply a validated plan into a mutable `LocaldConfig`.
///
/// For Phase 29.1.3, we support:
/// - `declare-service`: adds a new service to the config (fails if it already exists)
/// - `allocate-port`: no-op, but validates referenced service exists
///
/// Other ops are rejected.
pub fn apply_plan_to_config(
    config: &mut LocaldConfig,
    plan: &Plan,
    caps: &HostCapabilities,
) -> std::result::Result<(), Diagnostics> {
    validate_plan(plan, caps)?;

    // Apply steps in topological order for future-proofing.
    let order = topo_order(plan).map_err(diagnostics_error)?;

    for step_id in order {
        let step = plan
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .ok_or_else(|| diagnostics_error("internal error: missing step during apply"))?;

        apply_step(config, step)?;
    }

    Ok(())
}

fn topo_order(plan: &Plan) -> Result<Vec<String>, String> {
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();

    for step in &plan.steps {
        indegree.insert(step.id.as_str(), 0);
        edges.insert(step.id.as_str(), Vec::new());
    }

    for step in &plan.steps {
        for need in &step.needs {
            let to = step.id.as_str();
            let from = need.as_str();
            edges.get_mut(from).unwrap().push(to);
            *indegree.get_mut(to).unwrap() += 1;
        }
    }

    let mut q = VecDeque::new();
    for (id, deg) in &indegree {
        if *deg == 0 {
            q.push_back(*id);
        }
    }

    let mut out = Vec::with_capacity(plan.steps.len());
    while let Some(id) = q.pop_front() {
        out.push(id.to_string());
        for succ in edges.get(id).unwrap() {
            let d = indegree.get_mut(succ).unwrap();
            *d -= 1;
            if *d == 0 {
                q.push_back(*succ);
            }
        }
    }

    if out.len() == plan.steps.len() {
        Ok(out)
    } else {
        Err("plan step graph contains a cycle".to_string())
    }
}

fn apply_step(config: &mut LocaldConfig, step: &Step) -> std::result::Result<(), Diagnostics> {
    match &step.op {
        Op::DeclareService(op) => apply_declare_service(config, op),
        Op::AllocatePort(op) => {
            // For now, locald already allocates ports as needed.
            if !config.services.contains_key(&op.name) {
                Err(diagnostics_error(format!(
                    "allocate-port references unknown service '{}'",
                    op.name
                )))
            } else {
                Ok(())
            }
        }
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

fn apply_declare_service(
    config: &mut LocaldConfig,
    op: &DeclareServiceOp,
) -> std::result::Result<(), Diagnostics> {
    let name = op.name.trim();
    if name.is_empty() {
        return Err(diagnostics_error("declare-service has an empty name"));
    }

    if config.services.contains_key(name) {
        return Err(diagnostics_error(format!(
            "declare-service attempted to create service '{name}' but it already exists"
        )));
    }

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
        let v = eval_expr_lit(expr).ok_or_else(|| {
            diagnostics_error(format!(
                "unsupported expression for setting '{key}': only literal values are supported"
            ))
        })?;

        // Common settings.
        if let Some(env_key) = key.strip_prefix("env.") {
            let env_key = env_key.trim();
            if env_key.is_empty() {
                return Err(diagnostics_error("env.* setting has empty key"));
            }
            let text = as_text(&v).ok_or_else(|| {
                diagnostics_error(format!("env.{env_key} must be a text literal"))
            })?;
            common.env.insert(env_key.to_string(), text.to_string());
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
                exec.image = Some(image.to_string());
                container.image = image.to_string();
            }
            "container_port" => {
                let port = as_u16(&v).ok_or_else(|| {
                    diagnostics_error("container_port must be an unsigned integer <= 65535")
                })?;
                exec.container_port = Some(port);
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

    config.services.insert(name.to_string(), svc);
    Ok(())
}

fn eval_expr_lit(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Lit(v) => Some(v.clone()),
        Expr::Get(_) => None,
    }
}

fn as_text(v: &Value) -> Option<&str> {
    match v {
        Value::Text(s) => Some(s.as_str()),
        _ => None,
    }
}

fn as_u16(v: &Value) -> Option<u16> {
    match v {
        Value::Unsigned(n) => u16::try_from(*n).ok(),
        Value::Signed(n) => u16::try_from(*n).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runner::AllocatePortOp;

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
        apply_plan_to_config(&mut cfg, &plan, &caps()).unwrap();

        let svc = cfg.services.get("web").expect("service inserted");
        match svc {
            ServiceConfig::Typed(TypedServiceConfig::Exec(exec)) => {
                assert_eq!(exec.command.as_deref(), Some("npm start"));
                assert_eq!(exec.common.port, Some(3000));
                assert_eq!(
                    exec.common.env.get("NODE_ENV").map(|s| s.as_str()),
                    Some("development")
                );
            }
            other => panic!("unexpected service config: {other:?}"),
        }
    }
}
