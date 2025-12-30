use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvLayerKind {
    Global,
    Context,
    Workspace,
    DotEnv,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLayerSource {
    pub kind: EnvLayerKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLayer {
    pub kind: EnvLayerKind,
    pub path: PathBuf,
    pub vars: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEnvVar {
    pub value: String,
    pub source: EnvLayerSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedEnv {
    pub vars: BTreeMap<String, ResolvedEnvVar>,
}

impl ResolvedEnv {
    pub fn get_value(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(|v| v.value.as_str())
    }
}

#[must_use]
pub fn merge_env_layers(layers: &[EnvLayer]) -> ResolvedEnv {
    let mut resolved = ResolvedEnv::default();

    for layer in layers {
        let source = EnvLayerSource {
            kind: layer.kind,
            path: layer.path.clone(),
        };

        for (key, value) in &layer.vars {
            resolved.vars.insert(
                key.clone(),
                ResolvedEnvVar {
                    value: value.clone(),
                    source: source.clone(),
                },
            );
        }
    }

    resolved
}

#[must_use]
pub fn overlay_env<S: BuildHasher>(
    base: &ResolvedEnv,
    overlay: &HashMap<String, String, S>,
    source: &EnvLayerSource,
) -> ResolvedEnv {
    let mut resolved = base.clone();

    for (key, value) in overlay {
        resolved.vars.insert(
            key.clone(),
            ResolvedEnvVar {
                value: value.clone(),
                source: source.clone(),
            },
        );
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(kind: EnvLayerKind, path: &str, pairs: &[(&str, &str)]) -> EnvLayer {
        EnvLayer {
            kind,
            path: PathBuf::from(path),
            vars: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn merge_env_layers_later_wins_and_provenance_tracks_source() {
        let layers = vec![
            layer(
                EnvLayerKind::Context,
                "/ctx/.locald.toml",
                &[("A", "1"), ("B", "1")],
            ),
            layer(
                EnvLayerKind::Workspace,
                "/ws/locald.workspace.toml",
                &[("B", "2")],
            ),
            layer(EnvLayerKind::DotEnv, "/proj/.env", &[("C", "3")]),
        ];

        let resolved = merge_env_layers(&layers);

        assert_eq!(resolved.get_value("A"), Some("1"));
        assert_eq!(resolved.get_value("B"), Some("2"));
        assert_eq!(resolved.get_value("C"), Some("3"));

        let b = resolved.vars.get("B").unwrap();
        assert_eq!(b.source.kind, EnvLayerKind::Workspace);
        assert_eq!(b.source.path, PathBuf::from("/ws/locald.workspace.toml"));
    }

    #[test]
    fn overlay_env_overrides_subset_and_preserves_existing_sources() {
        let base_layers = vec![layer(
            EnvLayerKind::Workspace,
            "/ws/locald.workspace.toml",
            &[("A", "1"), ("B", "1")],
        )];
        let base = merge_env_layers(&base_layers);

        let overlay: HashMap<String, String> = [("B".to_string(), "2".to_string())].into();
        let source = EnvLayerSource {
            kind: EnvLayerKind::Project,
            path: PathBuf::from("/proj/locald.toml"),
        };
        let resolved = overlay_env(&base, &overlay, &source);

        assert_eq!(resolved.get_value("A"), Some("1"));
        assert_eq!(resolved.get_value("B"), Some("2"));

        let a = resolved.vars.get("A").unwrap();
        assert_eq!(a.source.kind, EnvLayerKind::Workspace);

        let b = resolved.vars.get("B").unwrap();
        assert_eq!(b.source.kind, EnvLayerKind::Project);
    }
}
