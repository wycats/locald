use anyhow::Context;
use crossterm::style::Stylize;
use locald_core::attachments::{AttachmentSource, ManualCliSession, ProjectFilter, ProjectSection};
#[cfg(target_os = "macos")]
use locald_core::{DomainName, HostsFileSection};
use locald_core::{IpcRequest, IpcResponse, LocaldConfig};
use serde::Serialize;
use std::io::IsTerminal;

#[cfg(feature = "experimental-cnb")]
use crate::build;
#[cfg(target_os = "macos")]
use crate::cli::TrayCommands;
use crate::cli::{
    AddServiceType, AdminCommands, AiCommands, Cli, Commands, ConfigCommands, DebugCommands,
    ProjectCommands, RegistryCommands, ServerCommands, ServiceCommands, SurfaceCommands,
};
#[cfg(feature = "experimental-plugins")]
use crate::cli::{DistributionCommands, PluginCommands};
#[cfg(feature = "experimental-containers")]
use crate::container;
use crate::error::{CliError, CliResult, DaemonError};
use crate::{
    client, debug, doctor, global_config, hints, history, init, monitor, run, selfupgrade, service,
    style, trust, try_cmd, update_check, utils,
};
#[cfg(feature = "experimental-plugins")]
use crate::{distribution, plugin};

#[derive(Serialize)]
struct JsonServiceSummary {
    name: String,
    state: String,
    port: Option<u16>,
    url: Option<String>,
}

#[derive(Serialize)]
struct JsonServiceList {
    services: Vec<JsonServiceSummary>,
}

#[derive(Debug, Serialize)]
struct JsonServiceAction {
    service: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct JsonServiceActions {
    services: Vec<JsonServiceAction>,
}

#[derive(Serialize)]
struct JsonProjectAction {
    status: String,
}

fn project_stop_json_actions(config_path: &std::path::Path) -> CliResult<JsonServiceActions> {
    let config_content =
        std::fs::read_to_string(config_path).context("Failed to read locald.toml")?;
    let config: LocaldConfig =
        toml::from_str(&config_content).context("Failed to parse locald.toml")?;
    let services = config
        .services
        .keys()
        .map(|service_name| JsonServiceAction {
            service: format!("{}:{}", config.project.name, service_name),
            status: "stopped".to_owned(),
        })
        .collect();
    Ok(JsonServiceActions { services })
}

fn format_attachment_source(source: &AttachmentSource) -> String {
    match source {
        AttachmentSource::Editor { name, id, .. } => format!("editor:{name} ({id})"),
        AttachmentSource::CLI { pid } => format!("cli:{pid}"),
        AttachmentSource::ManualCLI(session) => {
            format!("cli:{} (manual session)", session.pid())
        }
        AttachmentSource::Runtime => "runtime (legacy)".to_string(),
        AttachmentSource::Pin => "pin".to_string(),
    }
}

const fn section_label(section: ProjectSection) -> &'static str {
    match section {
        ProjectSection::Active => "active",
        ProjectSection::AlwaysOn => "always-on",
        ProjectSection::Recent => "recent",
    }
}

fn resolve_project_locator(path: &std::path::Path) -> CliResult<std::path::PathBuf> {
    locald_core::normalize_project_locator(path).map_err(|source| {
        CliError::message(format!(
            "Failed to resolve project path `{}`: {source}",
            path.display()
        ))
    })
}

fn prepare_up_start(
    project_locator: &std::path::Path,
    verbose: bool,
    manual_cli_session: Option<ManualCliSession>,
) -> CliResult<(std::path::PathBuf, IpcRequest)> {
    let project_path = resolve_project_locator(project_locator)?;
    let request = IpcRequest::Start {
        project_path: project_path.clone(),
        verbose,
        manual_cli_session,
    };
    Ok((project_path, request))
}

fn warn_if_daemon_identity_mismatch() {
    let Ok(cli_executable) = std::env::current_exe() else {
        return;
    };

    let Ok(IpcResponse::DaemonIdentity(identity)) =
        client::send_request(&IpcRequest::GetDaemonIdentity)
    else {
        return;
    };

    if let Some(warning) = hints::daemon_identity_mismatch_warning(
        env!("LOCALD_BUILD_VERSION"),
        &cli_executable,
        &identity,
    ) {
        eprintln!("{warning}");
    }
}

pub fn run(cli: Cli) -> CliResult<()> {
    match &cli.command {
        Commands::Init {
            from_distribution,
            name,
            target,
            no_scaffold,
            offline,
            yes,
            verbose,
        } => {
            #[cfg(feature = "experimental-plugins")]
            if let Some(source) = from_distribution {
                distribution::init_from_distribution(
                    source,
                    name.as_deref(),
                    target.as_deref(),
                    *no_scaffold,
                    *offline,
                    *yes,
                    *verbose,
                )?;
                return Ok(());
            }
            #[cfg(not(feature = "experimental-plugins"))]
            {
                // Silence unused warnings when feature is disabled
                let _ = (name, target, no_scaffold, offline, yes, verbose);
                if from_distribution.is_some() {
                    return Err(CliError::message(
                        "--from-distribution requires the experimental-plugins feature",
                    ));
                }
            }
            init::run()?;
        }
        #[cfg(feature = "experimental-cnb")]
        Commands::Build {
            path,
            builder,
            buildpack,
            verbose,
        } => {
            build::run(path, builder, buildpack, *verbose)?;
        }
        Commands::Try { command } => {
            utils::ensure_daemon_running()?;
            try_cmd::run_adhoc(command.join(" "))?;
        }
        Commands::Exec { service, command } => {
            utils::ensure_daemon_running()?;
            run::run_task(service, command)?;
        }
        Commands::Add {
            command,
            name,
            port,
        } => {
            utils::ensure_daemon_running()?;
            // Check if the first argument is "postgres" to route to postgres handler
            if !command.is_empty() && command[0].to_lowercase() == "postgres" {
                // Extract postgres name from remaining args (default to "db")
                let pg_name = command.get(1).map(String::as_str).unwrap_or("db");
                service::add_postgres(pg_name, None)?;
            } else {
                let cmd_str = if command.len() == 1 && command[0] == "last" {
                    history::get_last().context("No history found")?
                } else {
                    command.join(" ")
                };
                service::add_exec(cmd_str, name.clone(), *port)?;
            }
        }
        Commands::Service { command } => match command {
            ServiceCommands::Add { service_type } => match service_type {
                AddServiceType::Exec {
                    command,
                    name,
                    port,
                } => {
                    utils::ensure_daemon_running()?;
                    service::add_exec(command.join(" "), name.clone(), *port)?;
                }
                AddServiceType::Postgres { name, version } => {
                    utils::ensure_daemon_running()?;
                    service::add_postgres(name, version.clone())?;
                }
                AddServiceType::Container {
                    image,
                    name,
                    container_port,
                    command,
                } => {
                    utils::ensure_daemon_running()?;
                    service::add_container(
                        image.clone(),
                        name.clone(),
                        *container_port,
                        command.clone(),
                    )?;
                }
                AddServiceType::Site {
                    path,
                    name,
                    port,
                    build,
                } => {
                    utils::ensure_daemon_running()?;
                    service::add_site(path, name.clone(), *port, build.clone())?;
                }
            },
            ServiceCommands::Reset { name } => {
                utils::ensure_daemon_running()?;
                // Resolve full name if needed
                let full_name = {
                    let config_path = std::env::current_dir()?.join("locald.toml");
                    if config_path.exists() {
                        std::fs::read_to_string(&config_path).map_or_else(
                            |_| name.clone(),
                            |content| {
                                toml::from_str::<LocaldConfig>(&content).map_or(name.clone(), |c| {
                                    format!("{}:{}", c.project.name, name)
                                })
                            },
                        )
                    } else {
                        name.clone()
                    }
                };

                match client::send_request(&IpcRequest::Reset {
                    name: full_name.clone(),
                }) {
                    Ok(IpcResponse::Ok) => {
                        println!("{} Reset service {}", style::CHECK, full_name.bold());
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to reset {full_name}: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
        },
        Commands::Monitor => {
            utils::ensure_daemon_running()?;
            monitor::run()?;
        }
        Commands::Ping => {
            let response = client::send_request(&IpcRequest::Ping)?;
            println!("Received: {response:?}");
        }
        Commands::Trust => {
            trust::run()?;
        }
        Commands::Server { command } => match command {
            ServerCommands::Start => {
                // Run the server logic directly
                // The server will use the shim (via shim_client) to bind privileged ports if needed.
                let version = env!("LOCALD_BUILD_VERSION").to_string();
                locald_server::run(true, version)?;
            }
            ServerCommands::Shutdown => {
                let response = client::send_request(&IpcRequest::Shutdown)?;
                println!("{response:?}");
            }
            ServerCommands::Restart => {
                match client::send_request(&IpcRequest::Shutdown) {
                    Ok(_) => println!("Shutting down locald..."),
                    Err(e) => {
                        if !matches!(
                            e,
                            CliError::Daemon(
                                DaemonError::NotRunning { .. }
                                    | DaemonError::ConnectionRefused { .. }
                            )
                        ) {
                            return Err(e);
                        }
                    }
                }

                // Wait for shutdown
                for _ in 0..50 {
                    if client::send_request(&IpcRequest::Ping).is_err() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                println!("Starting locald server...");
                utils::spawn_daemon()?;
                println!("{} locald restarted successfully.", style::CHECK);
            }
        },
        Commands::Selfupgrade { check, version } => {
            if *check {
                match selfupgrade::check()? {
                    Some(latest) => {
                        println!(
                            "Update available: v{} (current: v{})",
                            latest,
                            selfupgrade::CURRENT_VERSION
                        );
                    }
                    None => {
                        println!("You are up to date (v{}).", selfupgrade::CURRENT_VERSION);
                    }
                }
            } else {
                selfupgrade::upgrade(version.as_deref())?;
            }
        }
        Commands::Up {
            path,
            verbose,
            exit_after_register,
        } => {
            let config = global_config::load();
            let update_rx = if config.updates.auto_check {
                let (tx, rx) = std::sync::mpsc::channel();
                update_check::spawn_update_check(move |result| {
                    let _ = tx.send(result);
                });
                Some(rx)
            } else {
                None
            };

            let report_update = |update_rx: &Option<std::sync::mpsc::Receiver<Option<String>>>| {
                if let Some(rx) = update_rx {
                    // Wait up to 500ms for update check to complete. This is long enough
                    // to catch fast responses while not noticeably delaying startup.
                    if let Ok(Some(new_version)) =
                        rx.recv_timeout(std::time::Duration::from_millis(500))
                    {
                        eprintln!(
                            "{} Update available: {} → {}. Run `locald selfupgrade`",
                            style::INFO,
                            selfupgrade::CURRENT_VERSION,
                            new_version
                        );
                    }
                }
            };

            let current_version = env!("LOCALD_BUILD_VERSION");

            // Check if already running and check version
            let should_restart = match client::send_request(&IpcRequest::GetVersion) {
                Ok(IpcResponse::Version(running_version)) => {
                    if running_version == current_version {
                        false
                    } else {
                        println!(
                            "Version mismatch (running: {}, current: {}). Restarting...",
                            running_version, current_version
                        );
                        true
                    }
                }
                Ok(_) => {
                    // Old version might not support GetVersion or returned something else (Pong?)
                    // If we sent GetVersion and got Pong, it means it deserialized as something else?
                    // Actually, if we send GetVersion to an old server, it might fail to deserialize the enum variant.
                    // Or if we sent Ping, we get Pong.
                    // Let's assume if we can't get version, we might want to restart if we are strict,
                    // but for now let's try to be safe.
                    // If the request fails (connection refused), it's not running.
                    false
                }
                Err(e) => {
                    // Not running or error
                    if matches!(
                        e,
                        CliError::Daemon(
                            DaemonError::NotRunning { .. } | DaemonError::ConnectionRefused { .. }
                        )
                    ) {
                        false
                    } else {
                        // Some other error, maybe restart?
                        false
                    }
                }
            };

            if should_restart {
                let _ = client::send_request(&IpcRequest::Shutdown);
                // Wait for shutdown
                for _ in 0..20 {
                    if client::send_request(&IpcRequest::Ping).is_err() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }

            // Check if running (again, in case we just shut it down or it wasn't running)
            let running = matches!(
                client::send_request(&IpcRequest::Ping),
                Ok(IpcResponse::Pong)
            );

            if running {
                cliclack::intro("locald up")?;
            } else {
                cliclack::intro("locald up")?;
                let s = cliclack::spinner();
                s.start("Starting locald server...");
                utils::spawn_daemon()?;
                s.stop("locald server started");
            }

            // Resolve path and check for config
            let target_path = if let Some(p) = path {
                p.clone()
            } else {
                std::env::current_dir()?
            };

            let config_exists = target_path.join("locald.toml").exists();

            // If no path was explicitly provided and no config exists, we are done.
            if path.is_none() && !config_exists {
                println!("{} Daemon is running.", style::CHECK);
                println!(
                    "No locald.toml found in current directory. Run `locald init` to create one."
                );
                report_update(&update_rx);
                return Ok(());
            }

            let manual_cli_session =
                (!*exit_after_register).then(|| ManualCliSession::new(std::process::id()));
            let (abs_path, start_request) =
                prepare_up_start(&target_path, *verbose, manual_cli_session)?;

            // Start services with streaming output.
            let mut attempts = 0;
            loop {
                match client::stream_boot_events(&start_request) {
                    Ok(()) => {
                        cliclack::outro("Project registered")?;
                        break;
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("Connection refused")
                            || err_str.contains("No such file or directory")
                        {
                            if attempts > 50 {
                                return Err(e);
                            }
                            attempts += 1;
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        } else {
                            cliclack::outro(format!("Failed to register project: {e}"))?;
                            return Err(e);
                        }
                    }
                }
            }

            report_update(&update_rx);

            // Test harnesses use the hidden flag so `locald up` can assert that
            // registration finished without staying attached to service logs.
            if *exit_after_register {
                return Ok(());
            }

            let detach_path = abs_path;
            let detach_source = manual_cli_session
                .context("log-following locald up did not create a Manual CLI session")?
                .attachment_source();
            let _ = ctrlc::set_handler(move || {
                // Best-effort detach on Ctrl+C
                let _ = client::send_request(&IpcRequest::ProjectDetach {
                    project_path: detach_path.clone(),
                    source: Some(detach_source.clone()),
                });
                std::process::exit(0);
            });

            println!("{} Streaming logs (Ctrl+C to stop)...", style::INFO);
            client::stream_logs(None, true)?;
        }
        Commands::Stop { name, json } => {
            if name.is_none() {
                let current_dir = std::env::current_dir()?;
                let config_path = current_dir.join("locald.toml");
                if !config_path.exists() {
                    return Err(CliError::message(
                        "No locald.toml found in current directory. Please specify a service name.",
                    ));
                }
                let json_actions = if *json {
                    Some(project_stop_json_actions(&config_path)?)
                } else {
                    None
                };
                let project_path = resolve_project_locator(&current_dir)?;
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::ProjectForceStop {
                    project_path: project_path.clone(),
                }) {
                    Ok(IpcResponse::Ok) => {
                        if let Some(json_actions) = json_actions {
                            println!("{}", serde_json::to_string_pretty(&json_actions)?);
                        } else {
                            println!("{} Paused project {}", style::CHECK, project_path.display());
                        }
                        return Ok(());
                    }
                    Ok(IpcResponse::Error(message)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to pause project: {message}",
                            style::CROSS
                        )));
                    }
                    Ok(response) => {
                        return Err(CliError::message(format!(
                            "Unexpected response: {response:?}"
                        )));
                    }
                    Err(error) => return Err(error),
                }
            }
            let Some(name) = name else {
                unreachable!("project stop returns before service-name resolution")
            };
            let names = vec![name.clone()];

            utils::ensure_daemon_running()?;
            let mut actions = Vec::new();

            for service_name in names {
                match client::send_request(&IpcRequest::Stop {
                    name: service_name.clone(),
                }) {
                    Ok(IpcResponse::Ok) => {
                        if *json {
                            actions.push(JsonServiceAction {
                                service: service_name.clone(),
                                status: "stopped".to_string(),
                            });
                        } else {
                            println!("{} Stopped service {}", style::CHECK, service_name.bold());
                        }
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to stop {service_name}: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }

            if *json {
                let json = serde_json::to_string_pretty(&JsonServiceActions { services: actions })?;
                println!("{json}");
            }
        }
        Commands::Restart { name, json } => {
            // Resolve full name if needed
            let full_name = {
                let config_path = std::env::current_dir()?.join("locald.toml");
                if config_path.exists() {
                    std::fs::read_to_string(&config_path).map_or_else(
                        |_| name.clone(),
                        |content| {
                            toml::from_str::<LocaldConfig>(&content)
                                .map_or(name.clone(), |c| format!("{}:{}", c.project.name, name))
                        },
                    )
                } else {
                    name.clone()
                }
            };

            match client::send_request(&IpcRequest::Restart {
                name: full_name.clone(),
            }) {
                Ok(IpcResponse::Ok) => {
                    if *json {
                        let response = JsonServiceActions {
                            services: vec![JsonServiceAction {
                                service: full_name,
                                status: "restarted".to_string(),
                            }],
                        };
                        let json = serde_json::to_string_pretty(&response)?;
                        println!("{json}");
                    } else {
                        println!("{} Restarted service {}", style::CHECK, full_name.bold());
                    }
                }
                Ok(IpcResponse::Error(msg)) => {
                    return Err(CliError::message(format!(
                        "{} Failed to restart {full_name}: {msg}",
                        style::CROSS
                    )));
                }
                Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                Err(e) => return Err(e),
            }
        }
        Commands::Status { json } => {
            utils::ensure_daemon_running()?;
            if !*json && std::io::stderr().is_terminal() {
                warn_if_daemon_identity_mismatch();
            }
            match client::send_request(&IpcRequest::Status) {
                Ok(IpcResponse::Status(services)) => {
                    if *json {
                        let summaries = services
                            .into_iter()
                            .map(|service| JsonServiceSummary {
                                name: service.name,
                                state: match service.status {
                                    locald_core::state::ServiceState::Running => {
                                        "running".to_string()
                                    }
                                    locald_core::state::ServiceState::Stopped => {
                                        "stopped".to_string()
                                    }
                                    locald_core::state::ServiceState::Building => {
                                        "building".to_string()
                                    }
                                },
                                port: service.port,
                                url: service.url,
                            })
                            .collect();
                        let json = serde_json::to_string_pretty(&JsonServiceList {
                            services: summaries,
                        })?;
                        println!("{json}");
                    } else if services.is_empty() {
                        println!("No services running.");
                    } else {
                        // Print table
                        println!(
                            "{:<20} {:<10} {:<10} {:<30}",
                            "NAME", "STATUS", "PORT", "URL"
                        );
                        for service in services {
                            let port_str = service
                                .port
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "-".to_string());
                            let url = service.url.unwrap_or_else(|| "-".to_string());
                            let status_style = match service.status {
                                locald_core::state::ServiceState::Running => {
                                    crossterm::style::Color::Green
                                }
                                locald_core::state::ServiceState::Stopped => {
                                    crossterm::style::Color::Red
                                }
                                locald_core::state::ServiceState::Building => {
                                    crossterm::style::Color::Blue
                                }
                            };
                            println!(
                                "{:<20} {:<10} {:<10} {:<30}",
                                service.name,
                                format!("{:?}", service.status).with(status_style),
                                port_str,
                                url
                            );

                            if !service.warnings.is_empty() {
                                println!(
                                    "  {} {}",
                                    "WARNING:".yellow().bold(),
                                    service.warnings.join(", ")
                                );
                            }
                        }
                    }
                }
                Ok(response) => {
                    return Err(CliError::message(format!(
                        "Unexpected response: {response:?}"
                    )));
                }
                Err(e) => return Err(e),
            }
        }
        Commands::Logs { service, follow } => {
            utils::ensure_daemon_running()?;
            let service_name = if let Some(name) = service {
                if name.contains(':') {
                    Some(name.clone())
                } else {
                    // Try to resolve project name
                    let config_path = std::env::current_dir()?.join("locald.toml");
                    if config_path.exists() {
                        std::fs::read_to_string(&config_path).map_or_else(
                            |_| Some(name.clone()),
                            |content| {
                                toml::from_str::<LocaldConfig>(&content)
                                    .map_or(Some(name.clone()), |c| {
                                        Some(format!("{}:{}", c.project.name, name))
                                    })
                            },
                        )
                    } else {
                        Some(name.clone())
                    }
                }
            } else {
                None
            };

            client::stream_logs(service_name, *follow)?;
        }
        Commands::Admin { command } => {
            match command {
                // `args` is used only on Linux; suppress warning on other platforms.
                #[allow(unused_variables)]
                AdminCommands::Setup => {
                    #[cfg(unix)]
                    if !nix::unistd::geteuid().is_root() {
                        use std::io::IsTerminal;
                        use std::os::unix::process::CommandExt;
                        use std::process::Command;

                        // `admin setup` requires root for privileged operations.
                        // On Linux: shim install, cgroup setup, port binding.
                        // On macOS: CA trust, privileged helper install.
                        if !std::io::stdin().is_terminal() {
                            return Err(CliError::message(
                                "This command requires root privileges. Re-run with `sudo locald admin setup`.",
                            ));
                        }

                        let exe_path = std::env::current_exe()
                            .context("Failed to resolve current executable path")?;

                        let args: Vec<_> = std::env::args_os().skip(1).collect();

                        // On Linux, prefer pkexec when polkit is available (GUI auth dialog).
                        #[cfg(target_os = "linux")]
                        if locald_utils::shim::is_polkit_available() {
                            eprintln!(
                                "{} Using polkit for privilege escalation (GUI auth dialog)...",
                                style::INFO
                            );
                            let err = Command::new("pkexec").arg(&exe_path).args(&args).exec();
                            eprintln!(
                                "{} pkexec failed ({err}), falling back to sudo...",
                                style::WARN
                            );
                        }

                        // Fall back to sudo (works on both Linux and macOS).
                        let err = Command::new("sudo")
                            .arg("--")
                            .arg(&exe_path)
                            .args(&args)
                            .exec();
                        return Err(CliError::message(format!(
                            "Failed to exec sudo for admin setup: {err}"
                        )));
                    }

                    #[cfg(target_os = "linux")]
                    {
                        const SHIM_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_SHIM_PATH"));

                        cliclack::intro("locald admin setup")?;

                        let exe_path = std::env::current_exe()?;
                        let exe_dir = exe_path.parent().context("Failed to get exe directory")?;
                        let shim_path = exe_dir.join("locald-shim");

                        {
                            let s = cliclack::spinner();
                            s.start("Installing privileged helper...");
                            locald_utils::shim::install(&shim_path, SHIM_BYTES)?;
                            s.stop("Privileged helper installed");
                        }

                        // Install polkit policy for GUI privilege escalation (optional).
                        // This enables `pkexec locald admin setup` to show a graphical auth dialog.
                        // Note: We delegate to the shim binary so the write runs in a known-privileged context.
                        {
                            let s = cliclack::spinner();
                            s.start("Installing polkit policy (optional)...");

                            if locald_utils::shim::is_polkit_available() {
                                let output = std::process::Command::new(&shim_path)
                                    .arg("admin")
                                    .arg("install-polkit")
                                    .output()
                                    .context("Failed to run locald-shim admin install-polkit")?;

                                if output.status.success() {
                                    s.stop("Polkit policy installed");
                                } else {
                                    let stderr = String::from_utf8_lossy(&output.stderr);
                                    s.stop(format!(
                                        "Polkit policy not installed: {}",
                                        stderr.trim()
                                    ));
                                }
                            } else {
                                s.stop("Polkit not available (skipped)");
                            }
                        }

                        // Best-effort: configure HTTPS Root CA + system trust during admin setup.
                        // This avoids requiring a separate step on fresh machines.
                        let mut trust_installed = false;
                        {
                            let s = cliclack::spinner();
                            s.start("Configuring HTTPS trust (optional)...");
                            match crate::trust::install_root_ca_into_trust_store() {
                                Ok(()) => {
                                    trust_installed = true;
                                    s.stop("HTTPS trust configured");
                                }
                                Err(_e) => {
                                    s.stop("HTTPS trust not configured");
                                }
                            }
                        }

                        if !trust_installed {
                            println!(
                                "{} HTTPS trust was not installed (optional). If your browser warns, re-run `locald admin setup`.",
                                style::WARN
                            );
                        }

                        {
                            let s = cliclack::spinner();
                            s.start("Configuring cgroup root...");
                            let output = std::process::Command::new(&shim_path)
                                .arg("admin")
                                .arg("cgroup")
                                .arg("setup")
                                .output()
                                .context("Failed to run locald-shim admin cgroup setup")?;

                            if !output.status.success() {
                                s.error("Cgroup setup failed");

                                let stderr = String::from_utf8_lossy(&output.stderr);
                                if !stderr.trim().is_empty() {
                                    eprintln!("{stderr}");
                                }

                                let mut remediation = String::new();
                                let stderr_lc = stderr.to_lowercase();
                                if stderr_lc.contains("permission denied")
                                    && stderr_lc.contains("/sys/fs/cgroup")
                                {
                                    remediation.push_str("\n\nThis looks like a containerized environment where cgroup v2 is mounted read-only. `locald admin setup` must be run on the host OS.");
                                }

                                return Err(CliError::message(format!(
                                    "locald-shim admin cgroup setup failed with status: {}{remediation}",
                                    output.status
                                )));
                            }
                            s.stop("Cgroup root configured");
                        }

                        {
                            use locald_utils::privileged::{AcquireConfig, Severity, Status};

                            let s = cliclack::spinner();
                            s.start("Verifying host readiness...");

                            let expected_version = option_env!("LOCALD_EXPECTED_SHIM_VERSION");
                            let report = locald_utils::privileged::collect_report(AcquireConfig {
                                verbose: false,
                                expected_shim_version: expected_version,
                                expected_shim_bytes: Some(SHIM_BYTES),
                            })?;

                            if report.has_critical_failures() {
                                s.error("Host is not ready");
                                println!(
                                    "{} Admin setup completed, but the host is still not ready.",
                                    style::CROSS
                                );

                                for p in report.problems.iter().filter(|p| {
                                    p.severity == Severity::Critical && p.status == Status::Fail
                                }) {
                                    println!("- {}", p.summary);
                                    if !p.remediation.is_empty() {
                                        println!("  Fix:");
                                        for cmd in &p.remediation {
                                            println!("    - {}", cmd);
                                        }
                                    }
                                }

                                println!("Run `locald doctor --verbose` for details.");
                                return Err(CliError::message("Host not ready"));
                            }

                            s.stop("Host readiness verified");
                        }

                        cliclack::outro("Setup complete")?;
                        println!("Next: run `locald up`.");
                    }

                    #[cfg(target_os = "macos")]
                    {
                        crate::macos_setup::run_setup()?;
                    }

                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                    {
                        return Err(CliError::message(
                            "Admin setup is not supported on this platform.",
                        ));
                    }

                    // Note: We don't setcap on locald anymore, because the shim handles it.
                    // But if the user runs locald directly without shim, it won't have caps.
                    // That's fine, the shim is the intended way for privileged ops.
                }
                #[allow(unused_variables)]
                AdminCommands::Teardown => {
                    #[cfg(unix)]
                    if !nix::unistd::geteuid().is_root() {
                        use std::io::IsTerminal;
                        use std::os::unix::process::CommandExt;
                        use std::process::Command;

                        if !std::io::stdin().is_terminal() {
                            return Err(CliError::message(
                                "This command requires root privileges. Re-run with `sudo locald admin teardown`.",
                            ));
                        }

                        let exe_path = std::env::current_exe()
                            .context("Failed to resolve current executable path")?;

                        let args: Vec<_> = std::env::args_os().skip(1).collect();

                        let err = Command::new("sudo")
                            .arg("--")
                            .arg(&exe_path)
                            .args(&args)
                            .exec();
                        return Err(CliError::message(format!(
                            "Failed to exec sudo for admin teardown: {err}"
                        )));
                    }

                    #[cfg(target_os = "macos")]
                    {
                        cliclack::intro("locald admin teardown (macOS)")?;

                        {
                            let s = cliclack::spinner();
                            s.start("Removing menu bar agent...");

                            match uninstall_launch_agent() {
                                Ok(()) => {
                                    s.stop("Menu bar agent removed (if installed)");
                                    println!(
                                        "{} LaunchAgent com.locald.agent removed (if present).",
                                        style::CHECK
                                    );
                                }
                                Err(e) => {
                                    s.stop(format!(
                                        "Menu bar agent removal failed: {e} (non-fatal)"
                                    ));
                                }
                            }

                            // Remove extracted agent binary.
                            if let Ok(agent_path) = locald_utils::agent::agent_path() {
                                if agent_path.exists() {
                                    let _ = std::fs::remove_file(&agent_path);
                                }
                            }
                        }

                        // Remove privileged helper.
                        {
                            let s = cliclack::spinner();
                            s.start("Removing privileged helper...");
                            match crate::macos_helper::remove() {
                                Ok(()) => s.stop("Privileged helper removed"),
                                Err(error) => {
                                    s.error(format!("Privileged helper removal failed: {error}"));
                                    return Err(CliError::message(format!(
                                        "Failed to remove privileged helper: {error}"
                                    )));
                                }
                            }
                        }

                        cliclack::outro("Teardown complete")?;
                    }

                    #[cfg(not(target_os = "macos"))]
                    {
                        return Err(CliError::message(
                            "Admin teardown is only supported on macOS. On Linux, use your package manager.",
                        ));
                    }
                }
                AdminCommands::SyncHosts => {
                    #[cfg(target_os = "macos")]
                    {
                        require_root_for_hosts_sync(nix::unistd::geteuid().is_root())?;
                        let sudo_uid = std::env::var("SUDO_UID").ok();
                        let daemon_uid = invoking_user_uid(sudo_uid.as_deref())?;
                        let response = client::send_request_from_uid(
                            &IpcRequest::GetHostsDomains,
                            daemon_uid,
                        )?;
                        match response {
                            IpcResponse::HostsDomains(domains) => {
                                sync_hosts_file(&HostsFileSection::new(), &domains)?;
                                println!("Hosts file updated.");
                            }
                            IpcResponse::Error(message) => {
                                return Err(CliError::message(message));
                            }
                            response => {
                                return Err(CliError::message(format!(
                                    "Unexpected response: {response:?}"
                                )));
                            }
                        }
                    }

                    #[cfg(not(target_os = "macos"))]
                    {
                        match client::send_request(&IpcRequest::SyncHosts)? {
                            IpcResponse::Ok => println!("Hosts file updated."),
                            IpcResponse::Error(message) => {
                                return Err(CliError::message(message));
                            }
                            response => {
                                return Err(CliError::message(format!(
                                    "Unexpected response: {response:?}"
                                )));
                            }
                        }
                    }
                }
            }
        }
        Commands::Tray { command } => {
            #[cfg(target_os = "macos")]
            {
                match command {
                    TrayCommands::Start => {
                        let plist = dirs::home_dir()
                            .context("Could not determine home directory")?
                            .join("Library/LaunchAgents/com.locald.agent.plist");
                        if !plist.exists() {
                            use std::io::IsTerminal;

                            if std::io::stdin().is_terminal() {
                                let run_setup = dialoguer::Confirm::new()
                                    .with_prompt("The tray agent requires admin setup. Run it now?")
                                    .default(true)
                                    .interact()
                                    .unwrap_or(false);

                                if run_setup {
                                    use std::os::unix::process::CommandExt;

                                    let exe_path = std::env::current_exe()
                                        .context("Failed to get executable path")?;
                                    #[allow(clippy::disallowed_methods)]
                                    let err = std::process::Command::new("sudo")
                                        .arg("--")
                                        .arg(&exe_path)
                                        .arg("admin")
                                        .arg("setup")
                                        .exec();
                                    return Err(CliError::message(format!(
                                        "Failed to run admin setup: {err}"
                                    )));
                                }
                            }

                            return Err(CliError::message(
                                "LaunchAgent not installed. Run `locald admin setup` first.",
                            ));
                        }

                        // Verify agent binary integrity and auto-update if outdated.
                        {
                            const AGENT_BYTES: &[u8] =
                                include_bytes!(env!("LOCALD_EMBEDDED_AGENT_PATH"));

                            let agent_path = locald_utils::agent::agent_path()?;
                            match locald_utils::agent::verify_integrity(&agent_path, AGENT_BYTES) {
                                Ok(true) => {}
                                Ok(false) => {
                                    eprintln!("{} Agent binary outdated, updating...", style::WARN);
                                    // Stop the running agent before overwriting the binary.
                                    #[allow(clippy::disallowed_methods)]
                                    let _ = std::process::Command::new("launchctl")
                                        .args(["stop", "com.locald.agent"])
                                        .output();
                                    locald_utils::agent::install(&agent_path, AGENT_BYTES)?;
                                }
                                Err(e) => {
                                    eprintln!("{} Failed to verify agent: {e}", style::WARN);
                                }
                            }
                        }

                        #[allow(clippy::disallowed_methods)]
                        let status = std::process::Command::new("launchctl")
                            .args(["start", "com.locald.agent"])
                            .status()
                            .context("Failed to run launchctl start")?;

                        if !status.success() {
                            return Err(CliError::message(format!(
                                "launchctl start failed with status: {status}"
                            )));
                        }
                    }
                    TrayCommands::Stop => {
                        #[allow(clippy::disallowed_methods)]
                        let status = std::process::Command::new("launchctl")
                            .args(["stop", "com.locald.agent"])
                            .status()
                            .context("Failed to run launchctl stop")?;

                        if !status.success() {
                            return Err(CliError::message(format!(
                                "launchctl stop failed with status: {status}"
                            )));
                        }
                    }
                    TrayCommands::Status => {
                        let plist_path = dirs::home_dir()
                            .context("Could not determine home directory")?
                            .join("Library/LaunchAgents/com.locald.agent.plist");
                        let pinned_daemon_path = read_launch_agent_daemon_path(&plist_path)?;

                        #[allow(clippy::disallowed_methods)]
                        let output = std::process::Command::new("launchctl")
                            .args(["list", "com.locald.agent"])
                            .output()
                            .context("Failed to run launchctl list")?;

                        if output.status.success() {
                            // launchctl list succeeds when loaded, but the agent
                            // may not be running. Check for a PID in the output.
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            if stdout.contains("\"PID\"") {
                                println!("locald tray agent is running");
                            } else {
                                println!("locald tray agent is loaded but not running");
                                println!("  Start it with: locald tray start");
                            }
                        } else {
                            println!("locald tray agent is not loaded");
                        }

                        match pinned_daemon_path {
                            Some(path) => println!("Pinned daemon: {}", path.display()),
                            None => println!("Pinned daemon: not configured"),
                        }
                    }
                    TrayCommands::Restart => {
                        let plist = dirs::home_dir()
                            .context("Could not determine home directory")?
                            .join("Library/LaunchAgents/com.locald.agent.plist");
                        if !plist.exists() {
                            return Err(CliError::message(
                                "LaunchAgent not installed. Run `locald admin setup` first.",
                            ));
                        }

                        #[allow(clippy::disallowed_methods)]
                        let stop_status = std::process::Command::new("launchctl")
                            .args(["stop", "com.locald.agent"])
                            .status()
                            .context("Failed to run launchctl stop")?;

                        if !stop_status.success() {
                            return Err(CliError::message(format!(
                                "launchctl stop failed with status: {stop_status}"
                            )));
                        }

                        #[allow(clippy::disallowed_methods)]
                        let start_status = std::process::Command::new("launchctl")
                            .args(["start", "com.locald.agent"])
                            .status()
                            .context("Failed to run launchctl start")?;

                        if !start_status.success() {
                            return Err(CliError::message(format!(
                                "launchctl start failed with status: {start_status}"
                            )));
                        }
                    }
                }
            }

            #[cfg(not(target_os = "macos"))]
            {
                let _ = command;
                return Err(CliError::message(
                    "Tray commands are only supported on macOS.",
                ));
            }
        }
        Commands::Ai { command } => match command {
            AiCommands::Schema => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::AiSchema) {
                    Ok(IpcResponse::AiSchema(schema)) => println!("{schema}"),
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            AiCommands::Context => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::AiContext) {
                    Ok(IpcResponse::AiContext(context)) => println!("{context}"),
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
        },
        Commands::Debug { command } => match command {
            DebugCommands::Port { port } => {
                debug::check_port(*port)?;
            }
            DebugCommands::Identity { json } => {
                let cli_executable = std::env::current_exe()?;
                let cli_version = env!("LOCALD_BUILD_VERSION");
                match client::send_request(&IpcRequest::GetDaemonIdentity) {
                    Ok(IpcResponse::DaemonIdentity(identity)) => {
                        if *json {
                            let output = serde_json::json!({
                                "cli": {
                                    "version": cli_version,
                                    "executable": cli_executable,
                                },
                                "daemon": identity,
                                "version_match": identity.version == cli_version,
                                "executable_match": hints::paths_refer_to_same_file(
                                    &identity.executable,
                                    &cli_executable,
                                ),
                            });
                            println!("{}", serde_json::to_string_pretty(&output)?);
                        } else {
                            println!("CLI:    {} ({})", cli_version, cli_executable.display());
                            println!(
                                "Daemon: {} ({}, pid {})",
                                identity.version,
                                identity.executable.display(),
                                identity.pid
                            );
                            if identity.version == cli_version {
                                println!("Version: match");
                            } else {
                                println!("Version: mismatch");
                            }
                            if hints::paths_refer_to_same_file(
                                &identity.executable,
                                &cli_executable,
                            ) {
                                println!("Executable: match");
                            } else {
                                println!("Executable: mismatch");
                            }
                        }
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
        },
        Commands::Config { command } => match command {
            ConfigCommands::Show { provenance } => {
                use locald_server::config_loader::ConfigLoader;
                let loader = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(ConfigLoader::load())?;

                if *provenance {
                    let cwd = std::env::current_dir().context("Failed to get current directory")?;

                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;

                    println!("[server]");
                    println!(
                        "sandbox = {}  (from {})",
                        loader.global.server.is_sandbox(),
                        loader.explain_global("server.sandbox")
                    );

                    if let Ok(report) = rt.block_on(loader.load_service_provenance_report(&cwd)) {
                        for (service_name, service) in report.services {
                            let has_any = service.command.is_some()
                                || service.workdir.is_some()
                                || service.port.is_some()
                                || service.depends_on.is_some();

                            if !has_any {
                                continue;
                            }

                            println!();
                            println!("[services.{service_name}]");

                            if let Some(field) = service.command {
                                println!(
                                    "command = {value:?}  (from {source})",
                                    value = field.value,
                                    source = field.source.display()
                                );
                            }

                            if let Some(field) = service.workdir {
                                println!(
                                    "workdir = {value:?}  (from {source})",
                                    value = field.value,
                                    source = field.source.display()
                                );
                            }

                            if let Some(field) = service.port {
                                println!(
                                    "port = {value}  (from {source})",
                                    value = field.value,
                                    source = field.source.display()
                                );
                            }

                            if let Some(field) = service.depends_on {
                                println!(
                                    "depends_on = {value:?}  (from {source})",
                                    value = field.value,
                                    source = field.source.display()
                                );
                            }
                        }
                    }

                    let report = rt.block_on(loader.load_env_provenance_report(&cwd))?;

                    println!();
                    println!("[env]");
                    for (key, var) in report.base.vars {
                        println!(
                            "{key} = {value:?}  (from {source})",
                            value = var.value,
                            source = var.source.path.display()
                        );
                    }

                    for (service_name, env) in report.services {
                        let overrides: Vec<_> = env
                            .vars
                            .iter()
                            .filter(|(_k, v)| {
                                matches!(v.source.kind, locald_core::config::EnvLayerKind::Project)
                            })
                            .collect();

                        if overrides.is_empty() {
                            continue;
                        }

                        println!();
                        println!("[services.{service_name}.env]");
                        for (key, var) in overrides {
                            println!(
                                "{key} = {value:?}  (from {source})",
                                value = var.value,
                                source = var.source.path.display()
                            );
                        }
                    }
                } else {
                    println!("{}", toml::to_string_pretty(&loader.global)?);
                }
            }
        },
        Commands::Doctor { json, verbose } => {
            let code = doctor::run(*json, *verbose)?;
            std::process::exit(code);
        }
        Commands::Dashboard => {
            utils::ensure_daemon_running()?;
            let url = "http://locald.localhost";
            println!("Opening dashboard at {}", url);

            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();

            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(url).spawn();

            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", url])
                .spawn();
        }
        Commands::Project { command } => match command {
            ProjectCommands::Attach {
                path,
                source,
                editor_name,
                editor_id,
                editor_pid,
                json,
            } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                let source = match source.as_deref() {
                    Some("editor") => {
                        let name = editor_name
                            .clone()
                            .ok_or_else(|| CliError::message("--editor-name is required"))?;
                        let id = editor_id
                            .clone()
                            .ok_or_else(|| CliError::message("--editor-id is required"))?;
                        AttachmentSource::Editor {
                            name,
                            id,
                            pid: *editor_pid,
                        }
                    }
                    Some("cli") | None => AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    Some(other) => {
                        return Err(CliError::message(format!(
                            "Unknown attachment source: {other}"
                        )));
                    }
                };

                match client::send_request(&IpcRequest::ProjectAttach {
                    project_path: abs_path,
                    source,
                    standalone: true,
                }) {
                    Ok(IpcResponse::Ok) => {
                        if *json {
                            let payload = JsonProjectAction {
                                status: "ok".to_string(),
                            };
                            println!("{}", serde_json::to_string_pretty(&payload)?);
                        } else {
                            println!("{} Attachment registered.", style::CHECK);
                        }
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to attach project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            ProjectCommands::Detach {
                path,
                source,
                editor_id,
            } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                let source = match source.as_deref() {
                    None => None,
                    Some("editor") => {
                        let id = editor_id
                            .clone()
                            .ok_or_else(|| CliError::message("--editor-id is required"))?;
                        Some(AttachmentSource::Editor {
                            name: String::new(),
                            id,
                            pid: None,
                        })
                    }
                    Some("cli") => Some(AttachmentSource::CLI {
                        pid: std::process::id(),
                    }),
                    Some(other) => {
                        return Err(CliError::message(format!(
                            "Unknown attachment source: {other}"
                        )));
                    }
                };

                match client::send_request(&IpcRequest::ProjectDetach {
                    project_path: abs_path,
                    source,
                }) {
                    Ok(IpcResponse::Ok) => {
                        println!("{} Attachment removed.", style::CHECK);
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to detach project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            ProjectCommands::Start { path } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                match client::send_request(&IpcRequest::ProjectForceStart {
                    project_path: abs_path,
                }) {
                    Ok(IpcResponse::Ok) => println!("{} Project force-start queued.", style::CHECK),
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to start project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            ProjectCommands::Stop { path } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                match client::send_request(&IpcRequest::ProjectForceStop {
                    project_path: abs_path,
                }) {
                    Ok(IpcResponse::Ok) => println!("{} Project force-stop queued.", style::CHECK),
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to stop project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            ProjectCommands::Status { path, json } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                match client::send_request(&IpcRequest::ProjectStatus {
                    project_path: abs_path,
                }) {
                    Ok(IpcResponse::ProjectStatus(info)) => {
                        if *json {
                            println!("{}", serde_json::to_string_pretty(&info)?);
                        } else {
                            println!("Path: {}", info.project_path.display());
                            if let Some(name) = info.project_name {
                                println!("Name: {name}");
                            }
                            println!("Running: {}", if info.is_running { "yes" } else { "no" });

                            if info.services.is_empty() {
                                println!("Services: none");
                            } else {
                                println!("Services:");
                                for service in info.services {
                                    println!("  - {service}");
                                }
                            }

                            if info.attachments.is_empty() {
                                println!("Attachments: none");
                            } else {
                                println!("Attachments:");
                                for attachment in info.attachments {
                                    let source = format_attachment_source(&attachment.source);
                                    println!("  - {source}");
                                }
                            }
                        }
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to fetch project status: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            ProjectCommands::List { json, filter } => {
                utils::ensure_daemon_running()?;
                let filter = match filter.as_deref() {
                    None => None,
                    Some("active") => Some(ProjectFilter::Active),
                    Some("pinned") => Some(ProjectFilter::Pinned),
                    Some("recent") => Some(ProjectFilter::Recent),
                    Some("all") => Some(ProjectFilter::All),
                    Some(other) => {
                        return Err(CliError::message(format!("Unknown filter: {other}")));
                    }
                };

                match client::send_request(&IpcRequest::ProjectList { filter }) {
                    Ok(IpcResponse::ProjectList(entries)) => {
                        if *json {
                            println!("{}", serde_json::to_string_pretty(&entries)?);
                        } else if entries.is_empty() {
                            println!("No projects found.");
                        } else {
                            println!("{:<10} {:<6} {:<40} NAME", "SECTION", "RUN", "PATH");
                            for entry in entries {
                                let run = if entry.is_running { "yes" } else { "no" };
                                let section = section_label(entry.section);
                                let name = entry.project_name.unwrap_or_default();
                                println!(
                                    "{:<10} {:<6} {:<40} {}",
                                    section,
                                    run,
                                    entry.project_path.display(),
                                    name
                                );
                            }
                        }
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to list projects: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
        },
        Commands::Registry { command } => match command {
            RegistryCommands::List => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::RegistryList) {
                    Ok(IpcResponse::RegistryList(projects)) => {
                        if projects.is_empty() {
                            println!("No projects registered.");
                        } else {
                            println!("{:<30} {:<10} {:<10}", "PATH", "NAME", "PINNED");
                            for project in projects {
                                println!(
                                    "{:<30} {:<10} {:<10}",
                                    project.path.display(),
                                    project.name.unwrap_or_default(),
                                    if project.pinned { "Yes" } else { "No" }
                                );
                            }
                        }
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to list registered projects: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            RegistryCommands::Pin { path } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                match client::send_request(&IpcRequest::RegistryPin {
                    project_path: abs_path,
                }) {
                    Ok(IpcResponse::Ok) => println!("{} Project pinned.", style::CHECK),
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to pin project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            RegistryCommands::Unpin { path } => {
                utils::ensure_daemon_running()?;
                let abs_path = resolve_project_locator(path)?;
                match client::send_request(&IpcRequest::RegistryUnpin {
                    project_path: abs_path,
                }) {
                    Ok(IpcResponse::Ok) => println!("{} Project unpinned.", style::CHECK),
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to unpin project: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            RegistryCommands::Clean => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::RegistryClean) {
                    Ok(IpcResponse::RegistryCleaned(count)) => {
                        println!(
                            "{} Forgot {} missing projects; project data was preserved.",
                            style::CHECK,
                            count
                        );
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        return Err(CliError::message(format!(
                            "{} Failed to clean registry: {msg}",
                            style::CROSS
                        )));
                    }
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
        },
        #[cfg(feature = "experimental-containers")]
        Commands::Container { command } => match command {
            crate::cli::ContainerCommands::Run {
                image,
                command,
                interactive,
                detached,
            } => {
                utils::ensure_daemon_running()?;
                container::run(image.clone(), command.clone(), *interactive, *detached)?;
            }
        },

        #[cfg(feature = "experimental-plugins")]
        Commands::Plugin { command } => match command {
            PluginCommands::Install {
                source,
                name,
                project,
                user,
                force,
            } => {
                plugin::install(source, name.clone(), *project, *user, *force)?;
            }
            PluginCommands::Inspect {
                plugin: plugin_arg,
                kind,
                name,
                depends_on,
                config,
                grant,
            } => {
                plugin::inspect(
                    plugin_arg,
                    kind,
                    name.as_deref(),
                    depends_on.as_deref(),
                    config,
                    grant,
                )?;
            }
            PluginCommands::Validate {
                plugin: plugin_arg,
                kind,
                name,
                depends_on,
                config,
                grant,
            } => {
                plugin::validate(
                    plugin_arg,
                    kind,
                    name.as_deref(),
                    depends_on.as_deref(),
                    config,
                    grant,
                )?;
            }
            PluginCommands::Create {
                source,
                output,
                manifest,
                dry_run,
                force,
                verbose,
            } => {
                plugin::create(
                    source,
                    output.as_deref(),
                    manifest.as_deref(),
                    *dry_run,
                    *force,
                    *verbose,
                )?;
            }
        },

        #[cfg(feature = "experimental-plugins")]
        Commands::Distribution { command } => match command {
            DistributionCommands::Create {
                source,
                output,
                manifest,
                include_remote,
                dry_run,
                force,
                verbose,
            } => {
                distribution::create(
                    source,
                    output.as_deref(),
                    manifest.as_deref(),
                    *include_remote,
                    *dry_run,
                    *force,
                    *verbose,
                )?;
            }
        },

        Commands::Serve { path, port, bind } => {
            let abs_path = std::fs::canonicalize(path).context("Failed to resolve path")?;
            if !abs_path.exists() {
                return Err(CliError::message(format!(
                    "Path does not exist: {}",
                    abs_path.display()
                )));
            }
            if !abs_path.is_dir() {
                return Err(CliError::message(format!(
                    "Path is not a directory: {}",
                    abs_path.display()
                )));
            }

            // Run the static server
            // We use a blocking call here because the CLI command is long-running
            let (tx, _) = tokio::sync::broadcast::channel(100);

            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(locald_server::static_server::run_static_server(
                    *port, bind, abs_path, tx,
                ))?;
        }

        Commands::Surface { command } => match command {
            SurfaceCommands::CliManifest => {
                use clap::CommandFactory;

                let manifest = crate::surface_manifest::from_clap_command(Cli::command());
                let json = serde_json::to_string_pretty(&manifest)?;
                println!("{json}");
            }
        },
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn read_launch_agent_daemon_path(
    plist_path: &std::path::Path,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    let content = match std::fs::read_to_string(plist_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("Failed to read LaunchAgent plist"),
    };
    Ok(parse_launch_agent_daemon_path(&content).map(std::path::PathBuf::from))
}

#[cfg(target_os = "macos")]
fn parse_launch_agent_daemon_path(plist: &str) -> Option<String> {
    let key_start = plist.find("<key>LOCALD_DAEMON_PATH</key>")?;
    let after_key = &plist[key_start..];
    let string_start = after_key.find("<string>")? + "<string>".len();
    let after_string = &after_key[string_start..];
    let string_end = after_string.find("</string>")?;
    let value = unescape_xml(&after_string[..string_end]);
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(target_os = "macos")]
fn unescape_xml(value: &str) -> String {
    let mut unescaped = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(amp_index) = rest.find('&') {
        unescaped.push_str(&rest[..amp_index]);
        rest = &rest[amp_index..];

        if let Some(stripped) = rest.strip_prefix("&amp;") {
            unescaped.push('&');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("&lt;") {
            unescaped.push('<');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("&gt;") {
            unescaped.push('>');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("&quot;") {
            unescaped.push('"');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("&apos;") {
            unescaped.push('\'');
            rest = stripped;
        } else {
            unescaped.push('&');
            rest = &rest['&'.len_utf8()..];
        }
    }

    unescaped.push_str(rest);
    unescaped
}

#[cfg(target_os = "macos")]
fn require_root_for_hosts_sync(is_root: bool) -> CliResult<()> {
    if is_root {
        Ok(())
    } else {
        Err(CliError::message(
            "This command requires root privileges. Run `sudo locald admin sync-hosts`.",
        ))
    }
}

#[cfg(target_os = "macos")]
fn invoking_user_uid(sudo_uid: Option<&str>) -> CliResult<u32> {
    let uid = sudo_uid
        .ok_or_else(|| {
            CliError::message(
                "Could not identify the invoking user. Run the requested `sudo locald admin ...` command from your user session.",
            )
        })?
        .parse::<u32>()
        .map_err(|_| CliError::message("SUDO_UID is not a valid user ID."))?;
    if uid == 0 {
        return Err(CliError::message(
            "Privileged locald administration requires a non-root invoking user.",
        ));
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
fn sync_hosts_file(hosts: &HostsFileSection, domains: &[DomainName]) -> CliResult<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let content = runtime
        .block_on(hosts.read())
        .context("Failed to read hosts file")?;
    let domains = domains.iter().map(ToString::to_string).collect::<Vec<_>>();
    let updated = hosts.update_content(&content, &domains);
    runtime
        .block_on(hosts.write(&updated))
        .context("Failed to write hosts file")?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn locald_up_resolves_a_missing_project_locator() {
        let directory = tempfile::tempdir().expect("create locator directory");
        let locator = directory.path().join("missing").join("..").join("project");
        let expected = std::fs::canonicalize(directory.path())
            .expect("canonicalize locator directory")
            .join("project");
        let (resolved, request) =
            prepare_up_start(&locator, false, None).expect("prepare missing project start");

        assert_eq!(resolved, expected);
        assert!(matches!(
            request,
            IpcRequest::Start { project_path, .. } if project_path == expected
        ));
    }

    #[test]
    fn locald_up_normalizes_symlinked_project_spelling() {
        let directory = tempfile::tempdir().expect("create locator directory");
        let real = directory.path().join("real");
        let alias = directory.path().join("alias");
        let project = real.join("project");
        std::fs::create_dir_all(&project).expect("create real project path");
        std::os::unix::fs::symlink(&real, &alias).expect("create locator symlink");
        let expected = std::fs::canonicalize(project).expect("canonicalize real project path");
        let (resolved, request) = prepare_up_start(&alias.join("project"), false, None)
            .expect("prepare project start through symlink spelling");

        assert_eq!(resolved, expected);
        assert!(matches!(
            request,
            IpcRequest::Start { project_path, .. } if project_path == expected
        ));
    }

    #[test]
    fn invalid_project_locator_error_names_the_requested_path() {
        let locator = std::path::Path::new("/invalid\0project");

        let error = resolve_project_locator(locator).expect_err("reject invalid locator");
        let source = locald_core::normalize_project_locator(locator)
            .expect_err("invalid locator has an I/O cause")
            .to_string();
        let message = error.to_string();
        assert!(message.contains("Failed to resolve project path"));
        assert!(message.contains(&locator.display().to_string()));
        assert_eq!(message.matches(&source).count(), 1);
    }

    #[test]
    fn normal_up_streams_a_start_paired_with_its_manual_cli_owner() {
        let project = std::path::Path::new("/projects/example");
        let session = ManualCliSession::new(std::process::id());

        let (_, request) = prepare_up_start(project, false, Some(session))
            .expect("prepare paired normal-up lifecycle request");
        assert!(matches!(
            request,
            IpcRequest::Start {
                manual_cli_session: Some(actual),
                ..
            } if actual == session
        ));
    }

    #[test]
    fn exit_after_register_streams_start_without_a_manual_cli_owner() {
        let project = std::path::Path::new("/projects/example");

        let (_, request) = prepare_up_start(project, false, None)
            .expect("prepare non-following lifecycle request");
        assert!(matches!(
            request,
            IpcRequest::Start {
                manual_cli_session: None,
                ..
            }
        ));
    }

    #[test]
    fn project_stop_json_requires_a_valid_config_before_mutation() {
        let directory = tempfile::tempdir().expect("create stop JSON directory");
        let config_path = directory.path().join("locald.toml");
        std::fs::write(&config_path, "[project\nmalformed")
            .expect("write malformed stop JSON config");

        let error = project_stop_json_actions(&config_path)
            .expect_err("malformed config must fail before project stop is sent");

        assert!(error.to_string().contains("Failed to parse locald.toml"));
    }

    #[test]
    fn launch_agent_plist_pins_daemon_path() {
        let plist = crate::macos_setup::render_launch_agent_plist(
            std::path::Path::new("/Applications/locald agent/locald-agent"),
            std::path::Path::new("/Users/me/bin/locald"),
        );

        assert!(plist.contains("<key>EnvironmentVariables</key>"));
        assert!(plist.contains("<key>LOCALD_DAEMON_PATH</key>"));
        assert_eq!(
            parse_launch_agent_daemon_path(&plist),
            Some("/Users/me/bin/locald".to_string())
        );
    }

    #[test]
    fn launch_agent_plist_escapes_xml_values() {
        let plist = crate::macos_setup::render_launch_agent_plist(
            std::path::Path::new("/Applications/A&B/locald-agent"),
            std::path::Path::new("/Users/me/<debug>/locald"),
        );

        assert!(plist.contains("/Applications/A&amp;B/locald-agent"));
        assert!(plist.contains("/Users/me/&lt;debug&gt;/locald"));
        assert_eq!(
            parse_launch_agent_daemon_path(&plist),
            Some("/Users/me/<debug>/locald".to_string())
        );
    }

    #[test]
    fn launch_agent_daemon_path_is_trimmed_when_parsed() {
        let plist = r#"
<plist version="1.0">
<dict>
    <key>LOCALD_DAEMON_PATH</key>
    <string>  /Users/me/bin/locald  </string>
</dict>
</plist>"#;

        assert_eq!(
            parse_launch_agent_daemon_path(plist),
            Some("/Users/me/bin/locald".to_string())
        );
    }

    #[test]
    fn launch_agent_daemon_path_is_absent_when_key_missing() {
        assert_eq!(parse_launch_agent_daemon_path("<plist></plist>"), None);
    }

    #[test]
    fn hosts_sync_requires_explicit_root() {
        let error = require_root_for_hosts_sync(false).expect_err("non-root sync must fail");

        assert!(error.to_string().contains("sudo locald admin sync-hosts"));
        require_root_for_hosts_sync(true).expect("root sync may continue");
    }

    #[test]
    fn hosts_sync_identifies_the_non_root_invoking_user() {
        assert_eq!(invoking_user_uid(Some("501")).expect("valid uid"), 501);
        assert!(invoking_user_uid(None).is_err());
        assert!(invoking_user_uid(Some("root")).is_err());
        assert!(invoking_user_uid(Some("0")).is_err());
    }

    #[test]
    fn hosts_sync_writes_the_daemon_owned_domain_set() {
        let directory = tempfile::tempdir().expect("create temporary hosts directory");
        let path = directory.path().join("hosts");
        std::fs::write(&path, "127.0.0.1 localhost\n").expect("write hosts fixture");
        let hosts = HostsFileSection::with_path(path.clone());

        sync_hosts_file(
            &hosts,
            &[
                "custom.example.test"
                    .parse()
                    .expect("valid custom project domain"),
                "docs.local"
                    .parse()
                    .expect("valid explicit legacy-spelling project domain"),
            ],
        )
        .expect("synchronize hosts fixture");

        let updated = std::fs::read_to_string(path).expect("read synchronized hosts fixture");
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert!(updated.contains("127.0.0.1 docs.local"));
        assert_eq!(updated.matches("# BEGIN locald").count(), 1);
    }
}

#[cfg(target_os = "macos")]
fn uninstall_launch_agent() -> anyhow::Result<()> {
    let label = "com.locald.agent";

    // Under sudo, resolve the real user's plist location and UID.
    let (user_home, target_uid) = if nix::unistd::geteuid().is_root() {
        if let Ok(sudo_user) = std::env::var("SUDO_USER")
            && let Ok(Some(user)) = nix::unistd::User::from_name(&sudo_user)
        {
            (Some(user.dir), Some(user.uid.as_raw()))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let plist_dir = if let Some(ref home) = user_home {
        home.join("Library/LaunchAgents")
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
            .join("Library/LaunchAgents")
    };

    let plist_path = plist_dir.join(format!("{label}.plist"));

    if plist_path.exists() {
        // Unload from the correct domain.
        #[allow(clippy::disallowed_methods)]
        if let Some(uid) = target_uid {
            let service_target = format!("gui/{uid}/{label}");
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &service_target])
                .output();
        } else {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(&plist_path)
                .output();
        }
        std::fs::remove_file(&plist_path)?;
    }

    Ok(())
}
