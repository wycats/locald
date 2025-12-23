use anyhow::Result;
use std::path::Path;

use serde_json::json;

pub mod plan;
pub mod runner;

pub use runner::{HostCapabilities, PluginApplyError, PluginRunner, ServiceSpec, WorkspaceContext};

pub use plan::{apply_plan_to_config, validate_plan};

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

#[cfg(test)]
mod tests {
    use super::normalized_plan_debug_json;
    use crate::plugins::runner::{DeclareServiceOp, Expr, Op, Plan, Selector, Step, Value};
    use serde_json::json;

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
