use crate::health::ReadinessRequirement;
use anyhow::{Context, Result};
use locald_core::ListenerName;
use locald_core::config::{
    CommonServiceConfig, EnvLayer, EnvLayerKind, EnvLayerSource, ExecServiceConfig, GlobalConfig,
    LocaldConfig, ProjectConfig, ResolvedEnv, ServiceConfig, TypedServiceConfig,
    WorkerServiceConfig, merge_env_layers, overlay_env,
};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use tracing::info;

pub(crate) const SERVICE_REFERENCE_PATTERN: &str = r"\$\{services\.([^}]+)\}";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceReference {
    pub(crate) service_name: String,
    pub(crate) field: String,
}

fn service_reference_field_exists(service: &ServiceConfig, field: &str) -> bool {
    if service.is_published() {
        return field == "origin";
    }
    match field {
        "host" => return true,
        "port" => return ReadinessRequirement::service_requires_port(service),
        "url" => {
            return !matches!(service, ServiceConfig::Typed(TypedServiceConfig::Worker(_)));
        }
        "origin" => {
            return !matches!(
                service,
                ServiceConfig::Typed(
                    TypedServiceConfig::Worker(_) | TypedServiceConfig::Postgres(_)
                )
            ) && service
                .domains()
                .is_none_or(|domains| domains.iter().any(|domain| !domain.starts_with("*.")));
        }
        _ => {}
    }
    if let Some(listener) = ListenerName::from_port_field(field) {
        return service
            .listeners()
            .iter()
            .any(|configured| configured == listener);
    }
    field
        .strip_prefix("generated.")
        .and_then(|field| field.strip_suffix(".path"))
        .is_some_and(|name| service.generated().contains_key(name))
}

pub(crate) fn resolve_service_reference<'a>(
    body: &str,
    services: impl IntoIterator<Item = (&'a str, &'a ServiceConfig)>,
) -> Result<ServiceReference> {
    let mut services = services.into_iter().collect::<Vec<_>>();
    services.sort_by_key(|(service_name, _)| *service_name);
    services.dedup_by_key(|(service_name, _)| *service_name);

    let (first_service, first_field) = body.split_once('.').with_context(|| {
        format!("service reference `${{services.{body}}}` is missing its field")
    })?;
    if services.iter().any(|(service_name, service)| {
        *service_name == first_service && service_reference_field_exists(service, first_field)
    }) {
        return Ok(ServiceReference {
            service_name: first_service.to_owned(),
            field: first_field.to_owned(),
        });
    }
    let mut shaped_candidates = services
        .iter()
        .filter_map(|(service_name, service)| {
            body.strip_prefix(*service_name)
                .and_then(|field| field.strip_prefix('.'))
                .filter(|field| service_reference_field_exists(service, field))
                .map(|field| ServiceReference {
                    service_name: (*service_name).to_owned(),
                    field: field.to_owned(),
                })
        })
        .collect::<Vec<_>>();
    shaped_candidates.sort_by(|left, right| left.service_name.cmp(&right.service_name));

    match shaped_candidates.as_slice() {
        [reference] => return Ok(reference.clone()),
        [_, _, ..] => {
            let candidates = shaped_candidates
                .iter()
                .map(|reference| format!("`{}`", reference.service_name))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "service reference `${{services.{body}}}` is ambiguous between configured services {candidates}"
            );
        }
        [] => {}
    }

    if services
        .iter()
        .any(|(service_name, _)| *service_name == body)
    {
        anyhow::bail!("service reference `${{services.{body}}}` is missing its field");
    }

    if services
        .iter()
        .any(|(service_name, _)| *service_name == first_service)
    {
        return Ok(ServiceReference {
            service_name: first_service.to_owned(),
            field: first_field.to_owned(),
        });
    }

    let mut prefix_candidates = services
        .iter()
        .filter_map(|(service_name, _)| {
            body.strip_prefix(*service_name)
                .and_then(|field| field.strip_prefix('.'))
                .filter(|field| !field.is_empty())
                .map(|field| ServiceReference {
                    service_name: (*service_name).to_owned(),
                    field: field.to_owned(),
                })
        })
        .collect::<Vec<_>>();
    prefix_candidates.sort_by(|left, right| left.service_name.cmp(&right.service_name));
    match prefix_candidates.as_slice() {
        [reference] => return Ok(reference.clone()),
        [_, _, ..] => {
            let candidates = prefix_candidates
                .iter()
                .map(|reference| format!("`{}`", reference.service_name))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "service reference `${{services.{body}}}` is ambiguous between configured services {candidates}"
            );
        }
        [] => {}
    }

    Ok(ServiceReference {
        service_name: first_service.to_owned(),
        field: first_field.to_owned(),
    })
}

pub(crate) fn resolve_owned_service_reference(
    body: &str,
    service_name: &str,
) -> Result<ServiceReference> {
    anyhow::ensure!(
        body != service_name,
        "service reference `${{services.{body}}}` is missing its field"
    );
    if let Some(field) = body
        .strip_prefix(service_name)
        .and_then(|field| field.strip_prefix('.'))
    {
        return Ok(ServiceReference {
            service_name: service_name.to_owned(),
            field: field.to_owned(),
        });
    }
    let (referenced_service, field) = body.split_once('.').with_context(|| {
        format!("service reference `${{services.{body}}}` is missing its field")
    })?;
    Ok(ServiceReference {
        service_name: referenced_service.to_owned(),
        field: field.to_owned(),
    })
}

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
        // Use BaseDirs::config_dir() + "locald/config.toml" to match the CLI's global_config module.
        directories::BaseDirs::new().map_or_else(
            || PathBuf::from("locald-config.toml"),
            |dirs| dirs.config_dir().join("locald/config.toml"),
        )
    }

    async fn load_global_config(path: &PathBuf) -> Result<GlobalConfig> {
        let mut config: GlobalConfig = if path.exists() {
            let content = tokio::fs::read_to_string(path).await?;
            toml::from_str(&content)?
        } else {
            GlobalConfig::default()
        };

        // Sandbox override from environment
        if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
            config.server.sandbox = true;
        }

        Ok(config)
    }

    #[must_use]
    pub fn explain_global(&self, key: &str) -> Provenance {
        // This is a bit manual for now. We could use a macro or something smarter later.
        match key {
            "server.sandbox" => {
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
        if !override_svc.common.listeners.is_empty() {
            base.common
                .listeners
                .clone_from(&override_svc.common.listeners);
        }
        for (name, generated) in &override_svc.common.generated {
            base.common
                .generated
                .insert(name.clone(), generated.clone());
        }
        if override_svc.common.domains.is_some() {
            base.common.domains.clone_from(&override_svc.common.domains);
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

        // Populate Workspace and Constellation info
        for (layer_config, layer_path) in &upstream_configs {
            let file_name = layer_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if file_name == "locald.workspace.toml" {
                let workspace_name = layer_config
                    .project
                    .as_ref()
                    .map(|p| p.name.clone())
                    .or_else(|| {
                        layer_path
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().to_string())
                    });
                config.project.workspace = workspace_name;
            } else if file_name == ".locald.toml" {
                let constellation_name = layer_config
                    .project
                    .as_ref()
                    .map(|p| p.name.clone())
                    .or_else(|| {
                        layer_path
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().to_string())
                    });
                config.project.constellation = constellation_name;
            }
        }

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
        let LocaldConfig {
            project,
            plugins: _,
            services,
            worktrees: _,
        } = project_config;
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
                TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Site(_)
                | TypedServiceConfig::Published(_) => None,
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
                TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Site(_)
                | TypedServiceConfig::Published(_) => None,
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
                            listeners: Vec::new(),
                            generated: BTreeMap::new(),
                            domains: None,
                            env: HashMap::new(),
                            depends_on: Vec::new(),
                            health_check: None,
                            stop_signal: None,
                        },
                        command: Some(command),
                        workdir: None,
                        build: None,
                    }))
                } else {
                    ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                        common: CommonServiceConfig {
                            port: None,
                            listeners: Vec::new(),
                            generated: BTreeMap::new(),
                            domains: None,
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
                workspace: None,
                constellation: None,
            },
            plugins: std::collections::HashMap::new(),
            services,
            worktrees: None,
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
        let re = regex::Regex::new(SERVICE_REFERENCE_PATTERN)?;

        for (k, v) in env {
            let mut new_val = v.clone();
            let mut replacements = Vec::new();
            // We need to collect captures first because of async closure
            let mut captures = Vec::new();
            for cap in re.captures_iter(v) {
                let body = cap
                    .get(1)
                    .context("service reference is missing its body")?
                    .as_str();
                let reference = resolve_service_reference(
                    body,
                    config
                        .services
                        .iter()
                        .map(|(name, service)| (name.as_str(), service)),
                )?;
                let range = cap
                    .get(0)
                    .context("service reference has no complete match")?
                    .range();
                captures.push((range, reference.service_name, reference.field));
            }

            for (range, service_name, field) in captures {
                let full_service_name = format!("{}:{}", config.project.name, service_name);
                let val = lookup_fn(full_service_name, field).await?;
                replacements.push((range, val));
            }

            replacements.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
            for (range, val) in replacements {
                new_val.replace_range(range, &val);
            }

            resolved.insert(k.clone(), new_val);
        }

        Ok(resolved)
    }

    /// Validate service references without consulting mutable runtime state.
    ///
    /// This is the declarative validation phase for `${services.name.field}`
    /// interpolation. Runtime host/port/URL values require a dependency and
    /// are resolved after that dependency becomes ready. Semantic `origin`
    /// values come from declarative domain ownership, so they may reference
    /// the current service or another service without creating a dependency.
    pub(crate) fn validate_env_references(
        env: &HashMap<String, String>,
        config: &LocaldConfig,
        consumer_name: &str,
    ) -> Result<()> {
        let re = regex::Regex::new(SERVICE_REFERENCE_PATTERN)?;
        let consumer = config.services.get(consumer_name).ok_or_else(|| {
            anyhow::anyhow!("cannot validate environment for unknown service `{consumer_name}`")
        })?;
        let mut dependencies = HashSet::new();
        let mut pending = consumer.depends_on().clone();
        while let Some(dependency) = pending.pop() {
            if dependencies.insert(dependency.clone()) {
                let dependency_config = config.services.get(&dependency).ok_or_else(|| {
                    anyhow::anyhow!(
                        "service `{consumer_name}` depends on unknown service `{dependency}`"
                    )
                })?;
                pending.extend(dependency_config.depends_on().iter().cloned());
            }
        }

        for (env_name, value) in env {
            for captures in re.captures_iter(value) {
                let body = captures
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("service reference is missing its body"))?
                    .as_str();
                let reference = resolve_service_reference(
                    body,
                    config
                        .services
                        .iter()
                        .map(|(name, service)| (name.as_str(), service)),
                )?;
                let service_name = reference.service_name.as_str();
                let field = reference.field.as_str();
                let service = config.services.get(service_name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "environment variable `{env_name}` references unknown service `{service_name}`"
                    )
                })?;

                if service.is_published() {
                    anyhow::ensure!(
                        field == "origin",
                        "environment variable `{env_name}` references `{service_name}.{field}`, but published services expose only their stable semantic `origin`"
                    );
                    continue;
                }

                let self_reference = service_name == consumer_name;
                let listener_reference = ListenerName::from_port_field(field);
                if let Some(generated_name) =
                    crate::generated_files::generated_name_from_path_field(field)
                {
                    anyhow::ensure!(
                        self_reference,
                        "environment variable `{env_name}` references private generated file `{service_name}.{field}`; generated paths are visible only to their owning service"
                    );
                    anyhow::ensure!(
                        service.supports_generated_files(),
                        "environment variable `{env_name}` references generated file `{generated_name}` on unsupported service `{service_name}`"
                    );
                    anyhow::ensure!(
                        service.generated().contains_key(generated_name),
                        "environment variable `{env_name}` references unknown generated file `{generated_name}` on service `{service_name}`"
                    );
                    continue;
                }

                match (field, listener_reference) {
                    ("host", None) => {
                        anyhow::ensure!(
                            !self_reference,
                            "environment variable `{env_name}` references `{service_name}.host`; self-service interpolation supports `port`, `origin`, and declared named listener ports"
                        );
                    }
                    ("port" | "url", None) => {
                        anyhow::ensure!(
                            !matches!(service, ServiceConfig::Typed(TypedServiceConfig::Worker(_))),
                            "environment variable `{env_name}` references `{service_name}.{field}`, but worker services have no {field}"
                        );
                        anyhow::ensure!(
                            !self_reference || field == "port",
                            "environment variable `{env_name}` references `{service_name}.{field}`; self-service interpolation supports `port`, `origin`, and declared named listener ports"
                        );
                    }
                    ("origin", None) => {
                        anyhow::ensure!(
                            !matches!(
                                service,
                                ServiceConfig::Typed(
                                    TypedServiceConfig::Worker(_) | TypedServiceConfig::Postgres(_)
                                )
                            ),
                            "environment variable `{env_name}` references `{service_name}.origin`, but that service type has no routable HTTP origin"
                        );
                        anyhow::ensure!(
                            service.domains().is_none_or(|domains| {
                                domains.iter().any(|domain| !domain.starts_with("*."))
                            }),
                            "environment variable `{env_name}` references `{service_name}.origin`, but that service declares no exact domain"
                        );
                    }
                    (_, Some(listener_name)) => {
                        anyhow::ensure!(
                            self_reference,
                            "environment variable `{env_name}` references private listener `{service_name}.{field}`; named listeners are visible only to their owning service"
                        );
                        anyhow::ensure!(
                            service
                                .listeners()
                                .iter()
                                .any(|configured| configured == listener_name),
                            "environment variable `{env_name}` references unknown listener `{listener_name}` on service `{service_name}`"
                        );
                    }
                    _ => anyhow::bail!(
                        "environment variable `{env_name}` references unknown field `{field}` on service `{service_name}`"
                    ),
                }

                if field != "origin" && !self_reference {
                    anyhow::ensure!(
                        dependencies.contains(service_name),
                        "environment variable `{env_name}` references service `{service_name}`, but service `{consumer_name}` does not depend on it; add `{service_name}` to `{consumer_name}.depends_on`"
                    );
                }
            }
        }

        Ok(())
    }

    /// Validate service names against the interpolation grammar.
    pub(crate) fn validate_service_names(config: &LocaldConfig) -> Result<()> {
        for service_name in config.services.keys() {
            anyhow::ensure!(
                !service_name.contains('}'),
                "service `{service_name}` has an invalid name: `}}` is reserved as the closing delimiter in `${{services.<service>.<field>}}` references; rename the service to omit `}}`"
            );
        }
        Ok(())
    }

    /// Validate private named-listener declarations independently of runtime
    /// allocation.
    pub(crate) fn validate_listener_declarations(config: &LocaldConfig) -> Result<()> {
        let valid_name = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,62}$")?;

        for (service_name, service) in &config.services {
            let listeners = service.listeners();
            if listeners.is_empty() {
                continue;
            }

            anyhow::ensure!(
                service.supports_named_listeners(),
                "service `{service_name}` declares named listeners, but only exec and worker services support them"
            );

            let mut seen = HashSet::new();
            for listener in listeners {
                anyhow::ensure!(
                    valid_name.is_match(listener),
                    "service `{service_name}` listener `{listener}` is invalid; listener names must start with a letter and contain only letters, digits, `_`, or `-` (maximum 63 characters)"
                );
                anyhow::ensure!(
                    seen.insert(listener),
                    "service `{service_name}` declares duplicate listener `{listener}`"
                );
            }
        }

        Ok(())
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
                let dependency = config.services.get(dep).ok_or_else(|| {
                    anyhow::anyhow!("Service '{name}' depends on unknown service '{dep}'")
                })?;
                anyhow::ensure!(
                    !service.is_published() && !dependency.is_published(),
                    "service dependency `{name}` -> `{dep}` is invalid because dependency edges involving published services are not supported"
                );

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

    #[test]
    fn service_reference_resolution_preserves_legacy_and_exact_dotted_names() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "reference-resolution"

[services.api]
command = "serve-api"
listeners = ["worker"]

[services."api.listeners.worker"]
command = "serve-dotted"

[services."api.port"]
command = "serve-port"

[services."api.worker"]
command = "serve-worker"
listeners = ["events"]

[services."api.worker".generated.runtime]
source = "runtime.json"
"#,
        )
        .expect("parse service-reference resolution config");
        let services = config
            .services
            .iter()
            .map(|(name, service)| (name.as_str(), service))
            .collect::<Vec<_>>();
        let legacy =
            resolve_service_reference("api.listeners.worker.port", services.iter().copied())
                .expect("declared first-dot listener reference remains stable");
        assert_eq!(
            legacy,
            ServiceReference {
                service_name: "api".to_owned(),
                field: "listeners.worker.port".to_owned(),
            }
        );
        let legacy_primary = resolve_service_reference("api.port", services.iter().copied())
            .expect("a configured exact name cannot hide a valid legacy field");
        assert_eq!(
            legacy_primary,
            ServiceReference {
                service_name: "api".to_owned(),
                field: "port".to_owned(),
            }
        );

        for (body, expected_field) in [
            ("api.worker.port", "port"),
            ("api.worker.listeners.events.port", "listeners.events.port"),
            (
                "api.worker.generated.runtime.path",
                "generated.runtime.path",
            ),
            ("api.worker.password", "password"),
        ] {
            let reference = resolve_service_reference(
                body,
                std::iter::once((
                    "api.worker",
                    config
                        .services
                        .get("api.worker")
                        .expect("dotted service config"),
                )),
            )
            .expect("unique dotted service reference");
            assert_eq!(reference.service_name, "api.worker");
            assert_eq!(reference.field, expected_field);
        }

        let unknown = resolve_service_reference("missing.url", services.iter().copied())
            .expect("unknown service remains available for declarative diagnostics");
        assert_eq!(unknown.service_name, "missing");
        assert_eq!(unknown.field, "url");
        let missing_field = resolve_service_reference("api.worker", services.iter().copied())
            .expect_err("an exact dotted service still requires a field");
        assert!(missing_field.to_string().contains("is missing its field"));
    }

    #[test]
    fn service_reference_resolution_uses_declared_fields_and_rejects_genuine_ambiguity() {
        let exact_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "exact-reference"

[services.api]
command = "serve-api"

[services."api.listeners.chat"]
command = "serve-chat"
"#,
        )
        .expect("parse exact dotted reference config");
        let exact = resolve_service_reference(
            "api.listeners.chat.port",
            exact_config
                .services
                .iter()
                .map(|(name, service)| (name.as_str(), service)),
        )
        .expect("undeclared legacy listener cannot hide an exact dotted service");
        assert_eq!(exact.service_name, "api.listeners.chat");
        assert_eq!(exact.field, "port");

        let portless_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "portless-reference"

[services."api.worker"]
type = "worker"
command = "serve-worker"
listeners = ["chat"]

[services."api.worker.listeners.chat"]
type = "worker"
command = "serve-chat-worker"
"#,
        )
        .expect("parse portless dotted reference config");
        let listener = resolve_service_reference(
            "api.worker.listeners.chat.port",
            portless_config
                .services
                .iter()
                .map(|(name, service)| (name.as_str(), service)),
        )
        .expect("a portless exact service cannot make the declared listener ambiguous");
        assert_eq!(listener.service_name, "api.worker");
        assert_eq!(listener.field, "listeners.chat.port");

        let ambiguous_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "ambiguous-reference"

[services."api.worker"]
command = "serve-worker"
listeners = ["chat"]

[services."api.worker.listeners.chat"]
command = "serve-chat"
"#,
        )
        .expect("parse ambiguous dotted reference config");
        let error = resolve_service_reference(
            "api.worker.listeners.chat.port",
            ambiguous_config
                .services
                .iter()
                .map(|(name, service)| (name.as_str(), service)),
        )
        .expect_err("two recognized exact service boundaries are ambiguous");
        let message = error.to_string();
        assert!(message.contains("is ambiguous"));
        assert!(message.contains("`api.worker`"));
        assert!(message.contains("`api.worker.listeners.chat`"));
    }

    #[test]
    fn service_reference_validation_rejects_missing_services_and_fields() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.db]
type = "postgres"

[services.web]
type = "worker"
command = "run-web"
depends_on = ["db"]
"#,
        )
        .expect("parse test config");

        let missing = HashMap::from([(
            "DATABASE_URL".to_owned(),
            "${services.missing.url}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&missing, &config, "web")
            .expect_err("missing service reference must fail");
        assert!(error.to_string().contains("unknown service `missing`"));

        let unknown_field = HashMap::from([(
            "DATABASE_URL".to_owned(),
            "${services.db.password}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&unknown_field, &config, "web")
            .expect_err("unknown service field must fail");
        assert!(error.to_string().contains("unknown field `password`"));
    }

    #[test]
    fn published_services_expose_only_semantic_origin_interpolation() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.workbench]
type = "published"
domains = ["workbench"]

[services.web]
command = "serve"
"#,
        )
        .expect("parse published interpolation config");

        let origin = HashMap::from([(
            "WORKBENCH_ORIGIN".to_owned(),
            "${services.workbench.origin}".to_owned(),
        )]);
        ConfigLoader::validate_env_references(&origin, &config, "web")
            .expect("published semantic origin does not create a runtime dependency");

        for field in ["port", "host", "url", "listeners.hmr.port"] {
            let env = HashMap::from([(
                "PRIVATE_ENDPOINT".to_owned(),
                format!("${{services.workbench.{field}}}"),
            )]);
            let error = ConfigLoader::validate_env_references(&env, &config, "web")
                .expect_err("published private runtime interpolation must fail");
            assert!(
                error
                    .to_string()
                    .contains("published services expose only their stable semantic `origin`"),
                "unexpected error for {field}: {error}"
            );
        }
    }

    #[test]
    fn startup_order_rejects_dependency_edges_to_published_services() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.workbench]
type = "published"

[services.web]
command = "serve"
depends_on = ["workbench"]
"#,
        )
        .expect("parse dependency candidate");

        let error = ConfigLoader::resolve_startup_order(&config)
            .expect_err("managed service must not depend on published service");
        assert!(
            error
                .to_string()
                .contains("dependency edges involving published")
        );
    }

    #[test]
    fn service_name_validation_reserves_only_the_reference_closing_delimiter() {
        let invalid: LocaldConfig = toml::from_str(
            r#"
[project]
name = "invalid-service-name"

[services."api}worker"]
command = "serve"
"#,
        )
        .expect("quoted service name remains valid TOML");
        let error = ConfigLoader::validate_service_names(&invalid)
            .expect_err("reference closing delimiter must be reserved");
        let message = error.to_string();
        assert!(message.contains("service `api}worker` has an invalid name"));
        assert!(message.contains("reserved as the closing delimiter"));
        assert!(message.contains("rename the service to omit `}`"));

        let valid: LocaldConfig = toml::from_str(
            r#"
[project]
name = "valid-service-names"

[services."api.worker"]
command = "serve-dotted"

[services."worker:blue"]
command = "serve-colon"
"#,
        )
        .expect("parse accepted service names");
        ConfigLoader::validate_service_names(&valid)
            .expect("dots, colons, and other existing service-name characters remain valid");
    }

    #[tokio::test]
    async fn dotted_service_references_validate_and_resolve_exact_names() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services."api.worker"]
command = "serve"
listeners = ["events"]

[services."api.worker".generated.runtime]
source = "config/runtime.json"

[services."api.worker".env]
PRIMARY = "${services.api.worker.port}"
EVENTS = "${services.api.worker.listeners.events.port}"
RUNTIME = "${services.api.worker.generated.runtime.path}"
"#,
        )
        .expect("parse dotted service config");
        crate::generated_files::validate_declarations(&config)
            .expect("dotted owner has valid generated declarations");
        ConfigLoader::validate_env_references(
            config.services["api.worker"].env(),
            &config,
            "api.worker",
        )
        .expect("dotted owner references resolve declaratively");

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let resolved = ConfigLoader::resolve_env(config.services["api.worker"].env(), &config, {
            let calls = std::sync::Arc::clone(&calls);
            move |service, field| {
                let calls = std::sync::Arc::clone(&calls);
                async move {
                    calls
                        .lock()
                        .expect("record dotted service lookup")
                        .push((service, field.clone()));
                    let value = match field.as_str() {
                        "port" => "4100",
                        "listeners.events.port" => "4200",
                        "generated.runtime.path" => "/tmp/runtime.json",
                        other => anyhow::bail!("unexpected dotted service field {other}"),
                    };
                    Ok(value.to_owned())
                }
            }
        })
        .await
        .expect("resolve dotted service environment");
        assert_eq!(resolved["PRIMARY"], "4100");
        assert_eq!(resolved["EVENTS"], "4200");
        assert_eq!(resolved["RUNTIME"], "/tmp/runtime.json");
        assert!(
            calls
                .lock()
                .expect("read dotted service lookups")
                .iter()
                .all(|(service, _)| service == "app:api.worker")
        );
    }

    #[test]
    fn service_reference_validation_accepts_runtime_fields_and_rejects_worker_urls() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.db]
type = "postgres"

[services.jobs]
type = "worker"
command = "run-jobs"

[services.web]
type = "worker"
command = "run-web"
depends_on = ["db", "jobs"]
"#,
        )
        .expect("parse test config");

        let valid = HashMap::from([(
            "DATABASE_URL".to_owned(),
            "${services.db.url}@${services.db.host}:${services.db.port}".to_owned(),
        )]);
        ConfigLoader::validate_env_references(&valid, &config, "web")
            .expect("supported runtime fields are valid");

        let worker_url =
            HashMap::from([("JOBS_URL".to_owned(), "${services.jobs.url}".to_owned())]);
        let error = ConfigLoader::validate_env_references(&worker_url, &config, "web")
            .expect_err("worker URL must fail before ownership publication");
        assert!(error.to_string().contains("worker services have no url"));

        let self_reference =
            HashMap::from([("SELF_URL".to_owned(), "${services.web.host}".to_owned())]);
        let error = ConfigLoader::validate_env_references(&self_reference, &config, "web")
            .expect_err("self-reference must fail before ownership publication");
        assert!(
            error
                .to_string()
                .contains("self-service interpolation supports")
        );
    }

    #[test]
    fn named_listener_validation_accepts_owner_references_and_rejects_leaks() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "serve"
listeners = ["chat", "hmr-control"]

[services.worker]
type = "worker"
command = "work"
depends_on = ["web"]
"#,
        )
        .expect("parse named listener config");
        ConfigLoader::validate_listener_declarations(&config).expect("valid listener declarations");

        let owner_env = HashMap::from([
            ("PRIMARY_PORT".to_owned(), "${services.web.port}".to_owned()),
            (
                "CHAT_PORT".to_owned(),
                "${services.web.listeners.chat.port}".to_owned(),
            ),
            (
                "HMR_PORT".to_owned(),
                "${services.web.listeners.hmr-control.port}".to_owned(),
            ),
        ]);
        ConfigLoader::validate_env_references(&owner_env, &config, "web")
            .expect("own primary and listener ports are valid");

        let leaked = HashMap::from([(
            "CHAT_PORT".to_owned(),
            "${services.web.listeners.chat.port}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&leaked, &config, "worker")
            .expect_err("another service cannot inspect a private listener");
        assert!(
            error
                .to_string()
                .contains("visible only to their owning service")
        );

        let unknown = HashMap::from([(
            "UNKNOWN_PORT".to_owned(),
            "${services.web.listeners.missing.port}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&unknown, &config, "web")
            .expect_err("undeclared listener must fail");
        assert!(error.to_string().contains("unknown listener `missing`"));
    }

    #[test]
    fn named_listener_declarations_reject_invalid_duplicate_and_unsupported_entries() {
        let invalid: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "serve"
listeners = [".chat"]
"#,
        )
        .expect("parse invalid listener config structurally");
        let error = ConfigLoader::validate_listener_declarations(&invalid)
            .expect_err("invalid listener name must fail");
        assert!(error.to_string().contains("listener `.chat` is invalid"));

        let duplicate: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "serve"
listeners = ["chat", "chat"]
"#,
        )
        .expect("parse duplicate listener config structurally");
        let error = ConfigLoader::validate_listener_declarations(&duplicate)
            .expect_err("duplicate listener must fail");
        assert!(error.to_string().contains("duplicate listener `chat`"));

        let unsupported: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.db]
type = "postgres"
listeners = ["wire"]
"#,
        )
        .expect("parse unsupported listener config structurally");
        let error = ConfigLoader::validate_listener_declarations(&unsupported)
            .expect_err("managed database listener declaration must fail");
        assert!(error.to_string().contains("only exec and worker"));
    }

    #[test]
    fn generated_file_validation_accepts_owner_paths_and_rejects_private_leaks() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "serve"
listeners = ["chat"]

[services.web.generated.microfrontends]
source = "chat/microfrontends.jsonc"

[services.web.generated.microfrontends.replace]
"/applications/chat/development/local" = "${services.web.listeners.chat.port}"
"/options/localProxyPort" = "${services.web.port}"

[services.web.env]
MICROFRONTENDS_CONFIG = "${services.web.generated.microfrontends.path}"

[services.worker]
type = "worker"
command = "work"
"#,
        )
        .expect("parse generated file config");
        crate::generated_files::validate_declarations(&config)
            .expect("valid generated declarations");
        ConfigLoader::validate_env_references(config.services["web"].env(), &config, "web")
            .expect("own generated path is private and valid");

        let leaked = HashMap::from([(
            "CONFIG".to_owned(),
            "${services.web.generated.microfrontends.path}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&leaked, &config, "worker")
            .expect_err("another service cannot inspect a generated path");
        assert!(
            error
                .to_string()
                .contains("visible only to their owning service")
        );
    }

    #[test]
    fn generated_file_validation_rejects_cross_service_and_unsupported_owners() {
        let cross_service: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "serve"

[services.worker]
type = "worker"
command = "work"

[services.worker.generated.runtime]
source = "runtime.json"

[services.worker.generated.runtime.replace]
"/port" = "${services.web.port}"
"#,
        )
        .expect("parse cross-service generated config");
        let error = crate::generated_files::validate_declarations(&cross_service)
            .expect_err("cross-service generated replacement must fail");
        assert!(error.to_string().contains("owning service"));

        let worker_primary: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.worker]
type = "worker"
command = "work"

[services.worker.generated.runtime]
source = "runtime.json"

[services.worker.generated.runtime.replace]
"/port" = "${services.worker.port}"
"#,
        )
        .expect("parse worker primary-port generated config");
        let error = crate::generated_files::validate_declarations(&worker_primary)
            .expect_err("worker primary port must fail declaratively");
        assert!(
            error
                .to_string()
                .contains("worker has no configured or probe-assigned primary port")
        );

        let portful_worker: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.worker]
type = "worker"
command = "work"
port = 3000

[services.worker.generated.runtime]
source = "runtime.json"

[services.worker.generated.runtime.replace]
"/port" = "${services.worker.port}"
"#,
        )
        .expect("parse portful worker generated config");
        crate::generated_files::validate_declarations(&portful_worker)
            .expect("configured worker primary port is available to its generated file");

        for probe in ["http", "tcp"] {
            let probe_worker: LocaldConfig = toml::from_str(&format!(
                r#"
[project]
name = "app"

[services.worker]
type = "worker"
command = "work"
health_check = {{ type = "{probe}" }}

[services.worker.generated.runtime]
source = "runtime.json"

[services.worker.generated.runtime.replace]
"/port" = "${{services.worker.port}}"
"#
            ))
            .expect("parse probe-backed worker generated config");
            crate::generated_files::validate_declarations(&probe_worker)
                .expect("probe-assigned worker primary port is available to its generated file");
        }

        let unsupported: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.database]
type = "postgres"

[services.database.generated.runtime]
source = "runtime.json"
"#,
        )
        .expect("parse unsupported generated owner");
        let error = crate::generated_files::validate_declarations(&unsupported)
            .expect_err("postgres cannot own generated files");
        assert!(error.to_string().contains("only host exec and worker"));

        for service in [
            r#"
[services.web]
command = "serve"

[services.web.build]
builder = "builder"

[services.web.generated.runtime]
source = "runtime.json"
"#,
            r#"
[services.web]
type = "exec"
command = "serve"

[services.web.build]
builder = "builder"

[services.web.generated.runtime]
source = "runtime.json"
"#,
        ] {
            let built: LocaldConfig = toml::from_str(&format!(
                r#"
[project]
name = "app"
{service}
"#
            ))
            .expect("parse build-enabled generated owner");
            let error = crate::generated_files::validate_declarations(&built)
                .expect_err("build-enabled exec cannot consume a host generated path");
            assert!(
                error
                    .to_string()
                    .contains("explicit container mount contract")
            );
        }
    }

    #[test]
    fn semantic_origins_allow_self_and_cross_service_references_without_dependencies() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "v0"

[services.web]
command = "web"
domains = ["@"]

[services.frame]
command = "frame"
domains = ["frame", "*.frame"]
"#,
        )
        .expect("parse service domains");
        let web_env = HashMap::from([
            (
                "MAIN_ORIGIN".to_owned(),
                "${services.web.origin}".to_owned(),
            ),
            (
                "FRAME_ORIGIN".to_owned(),
                "${services.frame.origin}".to_owned(),
            ),
        ]);

        ConfigLoader::validate_env_references(&web_env, &config, "web")
            .expect("semantic origins do not create runtime dependency edges");
    }

    #[test]
    fn semantic_origin_requires_an_exact_routable_domain() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "web"
domains = ["*.preview"]

[services.worker]
type = "worker"
command = "worker"

[services.database]
type = "postgres"
"#,
        )
        .expect("parse services");

        let wildcard_only =
            HashMap::from([("ORIGIN".to_owned(), "${services.web.origin}".to_owned())]);
        let error = ConfigLoader::validate_env_references(&wildcard_only, &config, "worker")
            .expect_err("wildcard-only services have no canonical origin");
        assert!(error.to_string().contains("declares no exact domain"));

        let worker_origin =
            HashMap::from([("ORIGIN".to_owned(), "${services.worker.origin}".to_owned())]);
        let error = ConfigLoader::validate_env_references(&worker_origin, &config, "web")
            .expect_err("workers have no routable origin");
        assert!(error.to_string().contains("no routable HTTP origin"));

        let postgres_origin = HashMap::from([(
            "ORIGIN".to_owned(),
            "${services.database.origin}".to_owned(),
        )]);
        let error = ConfigLoader::validate_env_references(&postgres_origin, &config, "web")
            .expect_err("databases have no HTTP origin");
        assert!(error.to_string().contains("no routable HTTP origin"));
    }

    #[test]
    fn layered_exec_config_can_preserve_or_clear_domain_claims() {
        let mut base: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "base"
domains = ["@", "*.preview"]
"#,
        )
        .expect("parse base config");
        let preserve: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "preserve"
"#,
        )
        .expect("parse preserving override");
        ConfigLoader::merge_service_configs(&mut base.services, &preserve.services);
        assert_eq!(
            base.services["web"].domains(),
            Some(&["@".to_owned(), "*.preview".to_owned()] as &[String])
        );

        let clear: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
domains = []
"#,
        )
        .expect("parse clearing override");
        ConfigLoader::merge_service_configs(&mut base.services, &clear.services);
        assert_eq!(base.services["web"].domains(), Some(&[] as &[String]));
    }

    #[test]
    fn layered_exec_config_preserves_or_replaces_named_listeners() {
        let mut base: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "base"
listeners = ["upstream"]
"#,
        )
        .expect("parse base config");
        let preserve: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "preserve"
"#,
        )
        .expect("parse preserving override");
        ConfigLoader::merge_service_configs(&mut base.services, &preserve.services);
        assert_eq!(base.services["web"].listeners(), &["upstream"]);

        let replace: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
listeners = ["chat", "events"]
"#,
        )
        .expect("parse replacing override");
        ConfigLoader::merge_service_configs(&mut base.services, &replace.services);
        assert_eq!(base.services["web"].listeners(), &["chat", "events"]);
    }

    #[test]
    fn layered_exec_config_unions_generated_files_and_replaces_named_declarations() {
        let mut base: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "base"

[services.web.generated.shared]
source = "base.json"

[services.web.generated.base_only]
source = "base-only.json"
"#,
        )
        .expect("parse base generated config");
        let override_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
command = "override"

[services.web.generated.shared]
source = "override.jsonc"

[services.web.generated.override_only]
source = "override-only.json"
"#,
        )
        .expect("parse generated override");

        ConfigLoader::merge_service_configs(&mut base.services, &override_config.services);
        let generated = base.services["web"].generated();
        assert_eq!(generated["shared"].source, "override.jsonc");
        assert_eq!(generated["base_only"].source, "base-only.json");
        assert_eq!(generated["override_only"].source, "override-only.json");
    }

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
