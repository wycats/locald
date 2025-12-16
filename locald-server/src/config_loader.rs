use anyhow::{Context, Result};
use locald_core::config::{
    CommonServiceConfig, ExecServiceConfig, GlobalConfig, LocaldConfig, ProjectConfig,
    ServiceConfig, TypedServiceConfig, WorkerServiceConfig,
};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    Default,
    Global(PathBuf),
    Context(PathBuf),
    Workspace(PathBuf),
    Project(PathBuf),
    EnvVar(String),
}

impl std::fmt::Display for Provenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::Global(p) | Self::Context(p) | Self::Workspace(p) | Self::Project(p) => {
                write!(f, "{}", p.display())
            }
            Self::EnvVar(k) => write!(f, "env:{k}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigLoader {
    pub global: GlobalConfig,
    pub global_path: PathBuf,
}

impl ConfigLoader {
    pub async fn load() -> Result<Self> {
        let global_path = Self::global_config_path();
        let global = Self::load_global_config(&global_path).await?;
        Ok(Self {
            global,
            global_path,
        })
    }

    fn global_config_path() -> PathBuf {
        directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
            || PathBuf::from("locald-config.toml"),
            |dirs| dirs.config_dir().join("config.toml"),
        )
    }

    async fn load_global_config(path: &PathBuf) -> Result<GlobalConfig> {
        let mut config: GlobalConfig = if path.exists() {
            let content = tokio::fs::read_to_string(path).await?;
            toml::from_str(&content)?
        } else {
            GlobalConfig::default()
        };

        // Sandbox override
        if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
            config.server.privileged_ports = false;
            config.server.fallback_ports = true;
        }

        // Warn on deprecated env vars
        if std::env::var("LOCALD_PRIVILEGED_PORTS").is_ok() {
            warn!(
                "WARNING: LOCALD_PRIVILEGED_PORTS is deprecated and will be ignored. Use locald.toml or sandbox mode."
            );
        }
        if std::env::var("LOCALD_FALLBACK_PORTS").is_ok() {
            warn!(
                "WARNING: LOCALD_FALLBACK_PORTS is deprecated and will be ignored. Use locald.toml or sandbox mode."
            );
        }

        Ok(config)
    }

    #[must_use]
    pub fn explain_global(&self, key: &str) -> Provenance {
        // This is a bit manual for now. We could use a macro or something smarter later.
        match key {
            "server.privileged_ports" => {
                if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
                    Provenance::EnvVar("LOCALD_SANDBOX_ACTIVE".to_string())
                } else if self.global_path.exists() {
                    // We assume if the file exists, the value *might* come from there.
                    // To be precise, we'd need to check if the key is actually in the file.
                    // For now, let's say Global if file exists, else Default.
                    Provenance::Global(self.global_path.clone())
                } else {
                    Provenance::Default
                }
            }
            "server.fallback_ports" => {
                if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
                    Provenance::EnvVar("LOCALD_SANDBOX_ACTIVE".to_string())
                } else if self.global_path.exists() {
                    Provenance::Global(self.global_path.clone())
                } else {
                    Provenance::Default
                }
            }
            _ => Provenance::Default,
        }
    }

    /// Loads configuration for a project from a directory.
    ///
    /// Tries to load `locald.toml` first, falling back to `Procfile`.
    /// Also loads `.env` files if present.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Neither `locald.toml` nor `Procfile` exists.
    /// - The configuration file cannot be read or parsed.
    pub async fn load_project_config(
        path: &PathBuf,
    ) -> Result<(LocaldConfig, HashMap<String, String>)> {
        // 1. Load Workspace Config (Recursive)
        let workspace_env = Self::load_workspace_config(path).await?;

        // 2. Read Project Config
        let config_path = path.join("locald.toml");
        let procfile_path = path.join("Procfile");

        let config = if config_path.exists() {
            let config_content = tokio::fs::read_to_string(&config_path)
                .await
                .context("Failed to read locald.toml")?;
            info!("Parsing config content: {}", config_content);
            toml::from_str(&config_content).context("Failed to parse locald.toml")?
        } else if procfile_path.exists() {
            let procfile_content = tokio::fs::read_to_string(&procfile_path)
                .await
                .context("Failed to read Procfile")?;
            Self::parse_procfile(&procfile_content, path)
        } else {
            anyhow::bail!("No locald.toml or Procfile found in {}", path.display());
        };

        // 3. Load .env if exists
        let env_path = path.join(".env");
        let mut dot_env_vars = HashMap::new();
        if env_path.exists() {
            info!("Loading .env from {:?}", env_path);
            if let Ok(iter) = dotenvy::from_path_iter(&env_path) {
                for (k, v) in iter.flatten() {
                    dot_env_vars.insert(k, v);
                }
            }
        }

        // 4. Merge: Workspace -> .env
        // Workspace envs are defaults, .env overrides them.
        let mut final_env = workspace_env;
        final_env.extend(dot_env_vars);

        Ok((config, final_env))
    }

    /// Recursively walks up the directory tree to find `locald.workspace.toml` or `.locald.toml`.
    /// Returns a merged map of environment variables.
    async fn load_workspace_config(start_path: &PathBuf) -> Result<HashMap<String, String>> {
        let mut current = start_path.parent();
        let mut layers = Vec::new();

        while let Some(path) = current {
            // Stop at git root or home
            if path.join(".git").exists()
                || path == directories::UserDirs::new().unwrap().home_dir()
            {
                // Check for workspace config at the root
                let workspace_config = path.join("locald.workspace.toml");
                if workspace_config.exists() {
                    layers.push(workspace_config);
                }
                break;
            }

            // Check for context config
            let context_config = path.join(".locald.toml");
            if context_config.exists() {
                layers.push(context_config);
            }

            current = path.parent();
        }

        // Apply layers from top (root) to bottom (nearest parent)
        let mut merged_env = HashMap::new();
        for layer_path in layers.iter().rev() {
            if let Ok(content) = tokio::fs::read_to_string(layer_path).await {
                if let Ok(_config) = toml::from_str::<GlobalConfig>(&content) {
                    // We reuse GlobalConfig struct for now as it has [env]
                    // Ideally we'd have a dedicated WorkspaceConfig struct
                    // For now, let's assume the structure matches or we parse generic TOML
                    if let Ok(value) = content.parse::<toml::Table>() {
                        if let Some(env) = value.get("env").and_then(|v| v.as_table()) {
                            for (k, v) in env {
                                if let Some(s) = v.as_str() {
                                    merged_env.insert(k.clone(), s.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(merged_env)
    }

    #[must_use]
    pub fn parse_procfile(content: &str, path: &std::path::Path) -> LocaldConfig {
        let mut services = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((name, command)) = line.split_once(':') {
                let name = name.trim().to_string();
                let command = command.trim().to_string();

                let service_config = if name == "web" {
                    ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
                        common: CommonServiceConfig {
                            port: None, // Will be assigned
                            env: HashMap::new(),
                            depends_on: Vec::new(),
                            health_check: None,
                            stop_signal: None,
                        },
                        command: Some(command),
                        image: None,
                        container_port: None,
                        workdir: None,
                        build: None,
                    }))
                } else {
                    ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                        common: CommonServiceConfig {
                            port: None,
                            env: HashMap::new(),
                            depends_on: Vec::new(),
                            health_check: None,
                            stop_signal: None,
                        },
                        command,
                        workdir: None,
                    }))
                };

                services.insert(name, service_config);
            }
        }

        let project_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();

        LocaldConfig {
            project: ProjectConfig {
                name: project_name,
                domain: None,
            },
            services,
        }
    }

    /// Resolves environment variables with variable substitution.
    ///
    /// Supports `${services.service_name.field}` syntax to inject values
    /// from other services (e.g., ports, URLs).
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The regex for substitution fails to compile (unlikely).
    /// - A referenced service or field cannot be resolved by the `lookup_fn`.
    pub async fn resolve_env<F, Fut>(
        env: &HashMap<String, String>,
        config: &LocaldConfig,
        lookup_fn: F,
    ) -> Result<HashMap<String, String>>
    where
        F: Fn(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        let mut resolved = HashMap::new();
        let re = regex::Regex::new(r"\$\{services\.([^.]+)\.([^}]+)\}")?;

        for (k, v) in env {
            let mut new_val = v.clone();
            let mut replacements = Vec::new();
            // We need to collect captures first because of async closure
            let mut captures = Vec::new();
            for cap in re.captures_iter(v) {
                captures.push((
                    cap.get(0).map(|m| m.range()),
                    cap.get(1).map(|m| m.as_str().to_string()),
                    cap.get(2).map(|m| m.as_str().to_string()),
                ));
            }

            for (range_opt, service_name_opt, field_opt) in captures {
                if let (Some(range), Some(service_name), Some(field)) =
                    (range_opt, service_name_opt, field_opt)
                {
                    let full_service_name = format!("{}:{}", config.project.name, service_name);
                    let val = lookup_fn(full_service_name, field).await?;
                    replacements.push((range, val));
                }
            }

            replacements.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
            for (range, val) in replacements {
                new_val.replace_range(range, &val);
            }

            resolved.insert(k.clone(), new_val);
        }

        Ok(resolved)
    }

    pub fn resolve_startup_order(config: &LocaldConfig) -> Result<Vec<String>> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Initialize
        for name in config.services.keys() {
            in_degree.insert(name.clone(), 0);
            dependents.insert(name.clone(), Vec::new());
        }

        // Build graph
        for (name, service) in &config.services {
            for dep in service.depends_on() {
                if !config.services.contains_key(dep) {
                    anyhow::bail!("Service '{name}' depends on unknown service '{dep}'");
                }

                // dep -> name
                dependents
                    .get_mut(dep)
                    .ok_or_else(|| anyhow::anyhow!("Service {dep} not found in dependents map"))?
                    .push(name.clone());

                *in_degree.get_mut(name).ok_or_else(|| {
                    anyhow::anyhow!("Service {name} not found in in_degree map")
                })? += 1;
            }
        }

        // Find initial nodes (0 dependencies)
        for (name, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(name.clone());
            }
        }

        let mut sorted = Vec::new();
        while let Some(node) = queue.pop_front() {
            sorted.push(node.clone());

            if let Some(neighbors) = dependents.get(&node) {
                for neighbor in neighbors {
                    let degree = in_degree.get_mut(neighbor).ok_or_else(|| {
                        anyhow::anyhow!("Service {neighbor} not found in in_degree map")
                    })?;
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        if sorted.len() != config.services.len() {
            anyhow::bail!("Circular dependency detected in services");
        }

        Ok(sorted)
    }
}
