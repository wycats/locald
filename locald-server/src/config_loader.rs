use anyhow::{Context, Result};
use locald_core::config::{
    CommonServiceConfig, EnvLayer, EnvLayerKind, EnvLayerSource, ExecServiceConfig, GlobalConfig,
    LocaldConfig, ProjectConfig, ResolvedEnv, ServiceConfig, TypedServiceConfig,
    WorkerServiceConfig, merge_env_layers, overlay_env,
};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize)]
pub struct LayerConfig {
    pub project: Option<ProjectConfig>,
    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,
}

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

#[derive(Debug, Clone)]
pub struct EnvProvenanceReport {
    pub base: ResolvedEnv,
    pub services: std::collections::BTreeMap<String, ResolvedEnv>,
}

#[derive(Debug, Clone)]
pub struct ProvenancedField<T> {
    pub value: T,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceProvenance {
    pub command: Option<ProvenancedField<String>>,
    pub workdir: Option<ProvenancedField<String>>,
    pub port: Option<ProvenancedField<u16>>,
    pub depends_on: Option<ProvenancedField<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct ServiceProvenanceReport {
    pub services: BTreeMap<String, ServiceProvenance>,
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

    async fn load_upstream_configs(start_path: &PathBuf) -> Vec<(LayerConfig, PathBuf)> {
        let discovered = Self::discover_layers(start_path);
        let mut configs = Vec::new();

        for (path, _kind) in discovered {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                if let Ok(config) = toml::from_str::<LayerConfig>(&content) {
                    configs.push((config, path));
                }
            }
        }
        configs
    }

    fn merge_service_configs(
        base: &mut HashMap<String, ServiceConfig>,
        override_services: &HashMap<String, ServiceConfig>,
    ) {
        for (name, override_svc) in override_services {
            match base.get_mut(name) {
                Some(base_svc) => {
                    Self::merge_single_service(base_svc, override_svc);
                }
                None => {
                    base.insert(name.clone(), override_svc.clone());
                }
            }
        }
    }

    fn merge_single_service(base: &mut ServiceConfig, override_svc: &ServiceConfig) {
        match base {
            ServiceConfig::Legacy(b) => {
                if let ServiceConfig::Legacy(o) = override_svc {
                    Self::merge_exec_service(b, o);
                    return;
                }
            }
            ServiceConfig::Typed(TypedServiceConfig::Exec(b)) => {
                if let ServiceConfig::Typed(TypedServiceConfig::Exec(o)) = override_svc {
                    Self::merge_exec_service(b, o);
                    return;
                }
            }
            ServiceConfig::Typed(_) => {}
        }
        *base = override_svc.clone();
    }

    fn merge_exec_service(base: &mut ExecServiceConfig, override_svc: &ExecServiceConfig) {
        if let Some(cmd) = &override_svc.command {
            base.command = Some(cmd.clone());
        }
        if let Some(wd) = &override_svc.workdir {
            base.workdir = Some(wd.clone());
        }
        if let Some(port) = override_svc.common.port {
            base.common.port = Some(port);
        }
        if !override_svc.common.depends_on.is_empty() {
            base.common
                .depends_on
                .clone_from(&override_svc.common.depends_on);
        }
        for (k, v) in &override_svc.common.env {
            base.common.env.insert(k.clone(), v.clone());
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
        // 1. Load Global/Context/Workspace Config (Recursive)
        let workspace_env = Self::load_workspace_config(path).await?;
        let upstream_configs = Self::load_upstream_configs(path).await;

        // 2. Read Project Config
        let (mut config, _config_source_path) = Self::read_project_config(path).await?;

        // 3. Merge Upstream Services into Project Config
        let mut merged_services = HashMap::new();
        for (layer_config, _) in upstream_configs {
            Self::merge_service_configs(&mut merged_services, &layer_config.services);
        }
        Self::merge_service_configs(&mut merged_services, &config.services);
        config.services = merged_services;

        // 4. Load .env if exists
        let dot_env_vars = Self::read_dotenv(path);

        // 5. Merge: Workspace -> .env
        // Workspace envs are defaults, .env overrides them.
        let mut final_env = workspace_env;
        final_env.extend(dot_env_vars);

        Ok((config, final_env))
    }

    pub async fn load_env_provenance_report(&self, path: &PathBuf) -> Result<EnvProvenanceReport> {
        let (config, config_source_path) = match Self::read_project_config(path).await {
            Ok(v) => v,
            Err(_) => {
                let layers = Self::load_effective_env_layers(path).await?;
                let dotenv_layer = Self::dotenv_layer(path);
                let mut all_layers = layers;
                if let Some(layer) = dotenv_layer {
                    all_layers.push(layer);
                }

                return Ok(EnvProvenanceReport {
                    base: merge_env_layers(&all_layers),
                    services: std::collections::BTreeMap::new(),
                });
            }
        };

        let mut layers = Self::load_effective_env_layers(path).await?;
        if let Some(layer) = Self::dotenv_layer(path) {
            layers.push(layer);
        }

        let base = merge_env_layers(&layers);

        let mut services = std::collections::BTreeMap::new();
        let project_source = EnvLayerSource {
            kind: EnvLayerKind::Project,
            path: config_source_path,
        };

        for (name, svc) in &config.services {
            let resolved = overlay_env(&base, svc.env(), &project_source);
            services.insert(name.clone(), resolved);
        }

        Ok(EnvProvenanceReport { base, services })
    }

    pub async fn load_service_provenance_report(
        &self,
        path: &PathBuf,
    ) -> Result<ServiceProvenanceReport> {
        let upstream_configs = Self::load_upstream_configs(path).await;
        let (project_config, project_config_path) = Self::read_project_config(path).await?;

        // Collect all layers in order: Upstream -> Project
        let mut all_layers = upstream_configs;
        let LocaldConfig { project, services } = project_config;
        all_layers.push((
            LayerConfig {
                project: Some(project),
                services,
            },
            project_config_path,
        ));

        // Identify all service names
        let mut all_service_names = std::collections::HashSet::new();
        for (layer, _) in &all_layers {
            for name in layer.services.keys() {
                all_service_names.insert(name.clone());
            }
        }

        let mut services: BTreeMap<String, ServiceProvenance> = BTreeMap::new();

        for name in all_service_names {
            let mut prov = ServiceProvenance::default();

            // Walk layers to find provenance for each field
            for (layer, source_path) in &all_layers {
                if let Some(service) = layer.services.get(&name) {
                    // Check fields and update provenance if present
                    if let Some(command) = Self::service_command(service) {
                        prov.command = Some(ProvenancedField {
                            value: command,
                            source: source_path.clone(),
                        });
                    }
                    if let Some(workdir) = Self::service_workdir(service) {
                        prov.workdir = Some(ProvenancedField {
                            value: workdir,
                            source: source_path.clone(),
                        });
                    }
                    if let Some(port) = service.port() {
                        prov.port = Some(ProvenancedField {
                            value: port,
                            source: source_path.clone(),
                        });
                    }
                    let depends_on = service.depends_on();
                    if !depends_on.is_empty() {
                        prov.depends_on = Some(ProvenancedField {
                            value: depends_on.clone(),
                            source: source_path.clone(),
                        });
                    }
                }
            }
            services.insert(name, prov);
        }

        Ok(ServiceProvenanceReport { services })
    }

    fn service_command(service: &ServiceConfig) -> Option<String> {
        match service {
            ServiceConfig::Legacy(exec) => exec.command.clone(),
            ServiceConfig::Typed(typed) => match typed {
                TypedServiceConfig::Exec(exec) => exec.command.clone(),
                TypedServiceConfig::Worker(worker) => Some(worker.command.clone()),
                TypedServiceConfig::Container(container) => container.command.clone(),
                TypedServiceConfig::Postgres(_) | TypedServiceConfig::Site(_) => None,
            },
        }
    }

    fn service_workdir(service: &ServiceConfig) -> Option<String> {
        match service {
            ServiceConfig::Legacy(exec) => exec.workdir.clone(),
            ServiceConfig::Typed(typed) => match typed {
                TypedServiceConfig::Exec(exec) => exec.workdir.clone(),
                TypedServiceConfig::Worker(worker) => worker.workdir.clone(),
                TypedServiceConfig::Container(container) => container.workdir.clone(),
                TypedServiceConfig::Postgres(_) | TypedServiceConfig::Site(_) => None,
            },
        }
    }

    async fn read_project_config(path: &PathBuf) -> Result<(LocaldConfig, PathBuf)> {
        let config_path = path.join("locald.toml");
        let procfile_path = path.join("Procfile");

        if config_path.exists() {
            let config_content = tokio::fs::read_to_string(&config_path)
                .await
                .context("Failed to read locald.toml")?;
            info!("Parsing config content: {}", config_content);
            let config: LocaldConfig =
                toml::from_str(&config_content).context("Failed to parse locald.toml")?;
            Ok((config, config_path))
        } else if procfile_path.exists() {
            let procfile_content = tokio::fs::read_to_string(&procfile_path)
                .await
                .context("Failed to read Procfile")?;
            Ok((Self::parse_procfile(&procfile_content, path), procfile_path))
        } else {
            anyhow::bail!("No locald.toml or Procfile found in {}", path.display());
        }
    }

    fn read_dotenv(path: &PathBuf) -> HashMap<String, String> {
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

        dot_env_vars
    }

    fn dotenv_layer(path: &PathBuf) -> Option<EnvLayer> {
        let env_path = path.join(".env");
        if !env_path.exists() {
            return None;
        }

        let vars = Self::read_dotenv(path);
        if vars.is_empty() {
            return None;
        }

        Some(EnvLayer {
            kind: EnvLayerKind::DotEnv,
            path: env_path,
            vars,
        })
    }

    /// Recursively walks up the directory tree to find `locald.workspace.toml` or `.locald.toml`.
    /// Returns a merged map of environment variables.
    async fn load_workspace_config(start_path: &PathBuf) -> Result<HashMap<String, String>> {
        let layers = Self::load_effective_env_layers(start_path).await?;

        let mut merged_env = HashMap::new();
        for (k, v) in merge_env_layers(&layers).vars {
            merged_env.insert(k, v.value);
        }

        Ok(merged_env)
    }

    async fn load_effective_env_layers(start_path: &PathBuf) -> Result<Vec<EnvLayer>> {
        let discovered = Self::discover_layers(start_path);
        let mut layers = Vec::new();

        for (path, kind) in discovered {
            if let Some(layer) = Self::env_layer_from_file(kind, &path).await {
                layers.push(layer);
            }
        }
        Ok(layers)
    }

    fn discover_layers(start_path: &PathBuf) -> Vec<(PathBuf, EnvLayerKind)> {
        let mut layers = Vec::new();

        // Global
        let global_path = Self::global_config_path();
        if global_path.exists() {
            layers.push((global_path, EnvLayerKind::Global));
        }

        // Context & Workspace
        let mut current = start_path.parent();
        let home_dir = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
        let mut parent_layers = Vec::new();

        while let Some(path) = current {
            let is_git_root = path.join(".git").exists();
            let is_home = home_dir.as_ref().is_some_and(|h| h == path);

            let context_config = path.join(".locald.toml");
            if context_config.exists() {
                parent_layers.push((context_config, EnvLayerKind::Context));
            }

            if is_git_root || is_home {
                let workspace_config = path.join("locald.workspace.toml");
                if workspace_config.exists() {
                    parent_layers.push((workspace_config, EnvLayerKind::Workspace));
                }
                break;
            }
            current = path.parent();
        }
        parent_layers.reverse();
        layers.extend(parent_layers);
        layers
    }

    async fn env_layer_from_file(kind: EnvLayerKind, path: &PathBuf) -> Option<EnvLayer> {
        if !path.exists() {
            return None;
        }

        let content = tokio::fs::read_to_string(path).await.ok()?;
        let vars = Self::parse_env_table(&content);
        if vars.is_empty() {
            return None;
        }

        Some(EnvLayer {
            kind,
            path: path.clone(),
            vars,
        })
    }

    fn parse_env_table(content: &str) -> HashMap<String, String> {
        let Ok(value) = content.parse::<toml::Table>() else {
            return HashMap::new();
        };

        let mut env = HashMap::new();
        let Some(table) = value.get("env").and_then(|v| v.as_table()) else {
            return env;
        };

        for (k, v) in table {
            if let Some(s) = v.as_str() {
                env.insert(k.clone(), s.to_string());
            }
        }

        env
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn service_provenance_comes_from_project_config_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("locald.toml");

        let toml = r#"
[project]
name = "app"

[services.web]
command = "npm start"
workdir = "frontend"
port = 3000
depends_on = ["db"]

[services.db]
type = "postgres"
version = "15"
"#;

        tokio::fs::write(&config_path, toml)
            .await
            .expect("write locald.toml");

        let loader = ConfigLoader {
            global: GlobalConfig::default(),
            global_path: PathBuf::new(),
        };

        let report = loader
            .load_service_provenance_report(&dir.path().to_path_buf())
            .await
            .expect("service provenance report");

        let web = report.services.get("web").expect("web service present");
        assert_eq!(
            web.command.as_ref().map(|f| f.value.as_str()),
            Some("npm start")
        );
        assert_eq!(
            web.workdir.as_ref().map(|f| f.value.as_str()),
            Some("frontend")
        );
        assert_eq!(web.port.as_ref().map(|f| f.value), Some(3000));
        let depends_on: Option<Vec<&str>> = web
            .depends_on
            .as_ref()
            .map(|f| f.value.iter().map(String::as_str).collect());
        assert_eq!(depends_on, Some(vec!["db"]));
        assert_eq!(web.command.as_ref().map(|f| &f.source), Some(&config_path));
        assert_eq!(web.workdir.as_ref().map(|f| &f.source), Some(&config_path));
        assert_eq!(web.port.as_ref().map(|f| &f.source), Some(&config_path));
        assert_eq!(
            web.depends_on.as_ref().map(|f| &f.source),
            Some(&config_path)
        );

        let db = report.services.get("db").expect("db service present");
        assert!(db.command.is_none());
        assert!(db.workdir.is_none());
        assert!(db.port.is_none());
        assert!(db.depends_on.is_none());
    }

    #[tokio::test]
    async fn service_provenance_cascades_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Create .git directory to mark workspace root
        tokio::fs::create_dir(root.join(".git"))
            .await
            .expect("create .git");

        // Workspace config
        let workspace_path = root.join("locald.workspace.toml");
        let workspace_toml = r#"
[services.web]
command = "npm run dev"
port = 3000
"#;
        tokio::fs::write(&workspace_path, workspace_toml)
            .await
            .expect("write workspace");

        // Project config (subdirectory)
        let project_dir = root.join("app");
        tokio::fs::create_dir(&project_dir)
            .await
            .expect("create project dir");
        let project_path = project_dir.join("locald.toml");
        let project_toml = r#"
[project]
name = "app"

[services.web]
port = 4000
"#;
        tokio::fs::write(&project_path, project_toml)
            .await
            .expect("write project");

        let loader = ConfigLoader {
            global: GlobalConfig::default(),
            global_path: PathBuf::new(),
        };

        // Load config
        let (config, _) = ConfigLoader::load_project_config(&project_dir)
            .await
            .expect("load config");
        let web = config.services.get("web").expect("web service");

        // Check merged values
        // Command from workspace
        assert_eq!(
            ConfigLoader::service_command(web),
            Some("npm run dev".to_string())
        );
        // Port from project (override)
        assert_eq!(web.port(), Some(4000));

        // Check provenance
        let report = loader
            .load_service_provenance_report(&project_dir)
            .await
            .expect("provenance");
        let web_prov = report.services.get("web").expect("web provenance");

        assert_eq!(
            web_prov.command.as_ref().map(|f| f.value.as_str()),
            Some("npm run dev")
        );
        assert_eq!(
            web_prov.command.as_ref().map(|f| &f.source),
            Some(&workspace_path)
        );

        assert_eq!(web_prov.port.as_ref().map(|f| f.value), Some(4000));
        assert_eq!(
            web_prov.port.as_ref().map(|f| &f.source),
            Some(&project_path)
        );
    }
}
