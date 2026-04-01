use anyhow::Context;
use crossterm::style::Stylize;
use locald_core::{HostsFileSection, IpcRequest, IpcResponse, LocaldConfig};
use serde::Serialize;
use std::collections::HashSet;

#[cfg(feature = "experimental-cnb")]
use crate::build;
use crate::cli::{
    AddServiceType, AdminCommands, AiCommands, Cli, Commands, ConfigCommands, DebugCommands,
    RegistryCommands, ServerCommands, ServiceCommands, SurfaceCommands,
};
#[cfg(feature = "experimental-plugins")]
use crate::cli::{DistributionCommands, PluginCommands};
#[cfg(feature = "experimental-containers")]
use crate::container;
use crate::error::{CliError, CliResult, DaemonError};
use crate::{
    client, debug, doctor, global_config, history, init, monitor, run, selfupgrade, service, style,
    trust, try_cmd, update_check, utils,
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

#[derive(Serialize)]
struct JsonServiceAction {
    service: String,
    status: String,
}

#[derive(Serialize)]
struct JsonServiceActions {
    services: Vec<JsonServiceAction>,
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
        Commands::Ping => match client::send_request(&IpcRequest::Ping) {
            Ok(response) => println!("Received: {response:?}"),
            Err(e) => return Err(e),
        },
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
            ServerCommands::Shutdown => match client::send_request(&IpcRequest::Shutdown) {
                Ok(response) => println!("{response:?}"),
                Err(e) => return Err(e),
            },
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
        Commands::Up { path, verbose } => {
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

            let abs_path = std::fs::canonicalize(target_path).context("Failed to resolve path")?;

            // Retry loop for connection?
            let mut attempts = 0;
            loop {
                match client::stream_boot_events(&IpcRequest::Start {
                    project_path: abs_path.clone(),
                    verbose: *verbose,
                }) {
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
        }
        Commands::Stop { name, json } => {
            let names = if let Some(n) = name {
                vec![n.clone()]
            } else {
                let config_path = std::env::current_dir()?.join("locald.toml");
                if !config_path.exists() {
                    return Err(CliError::message(
                        "No locald.toml found in current directory. Please specify a service name.",
                    ));
                }
                let config_content =
                    std::fs::read_to_string(&config_path).context("Failed to read locald.toml")?;
                let config: LocaldConfig =
                    toml::from_str(&config_content).context("Failed to parse locald.toml")?;

                config
                    .services
                    .keys()
                    .map(|service_name| format!("{}:{}", config.project.name, service_name))
                    .collect()
            };

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
                        // On macOS: pfctl port forwarding rules.
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
                        cliclack::intro("locald admin setup (macOS)")?;

                        // Step 1: Generate and trust the Root CA certificate.
                        {
                            let s = cliclack::spinner();
                            s.start("Configuring HTTPS trust...");
                            match crate::trust::install_root_ca_into_trust_store() {
                                Ok(()) => {
                                    s.stop("HTTPS trust configured");
                                }
                                Err(e) => {
                                    s.error(format!("HTTPS trust failed: {e}"));
                                    return Err(CliError::message(format!(
                                        "HTTPS trust setup failed: {e}\n\
                                         This is required for browsers to trust locald's HTTPS certificates.\n\
                                         Make sure you're running with sudo: sudo locald admin setup"
                                    )));
                                }
                            }
                        }

                        // Step 2: Install pfctl port forwarding (80→8080, 443→8443).
                        {
                            let s = cliclack::spinner();
                            s.start("Configuring port forwarding (80 → 8080, 443 → 8443)...");
                            match crate::port_forward::macos::install() {
                                Ok(()) => {
                                    s.stop("Port forwarding configured");
                                }
                                Err(e) => {
                                    s.error(format!("Port forwarding failed: {e}"));
                                    return Err(CliError::message(format!(
                                        "Port forwarding setup failed: {e}\n\
                                         This is required for locald to serve on ports 80/443.\n\
                                         Make sure you're running with sudo: sudo locald admin setup"
                                    )));
                                }
                            }
                        }

                        // Step 3: Enable privileged ports in global config only if pfctl succeeded.
                        if crate::port_forward::macos::is_installed() {
                            let s = cliclack::spinner();
                            s.start("Updating configuration...");
                            let mut config = crate::global_config::load();
                            config.server.privileged_ports = true;
                            match crate::global_config::save(config) {
                                Ok(()) => {
                                    s.stop("Configuration updated (privileged_ports = true)");
                                }
                                Err(e) => {
                                    s.stop(format!("Config save failed: {e} (non-fatal)"));
                                }
                            }
                        }

                        // Step 4: Install locald-agent as a LaunchAgent (starts at login).
                        {
                            let s = cliclack::spinner();
                            s.start("Installing menu bar agent...");

                            let exe_dir = std::env::current_exe()?
                                .parent()
                                .context("Failed to get executable directory")?
                                .to_path_buf();
                            let agent_path = exe_dir.join("locald-agent");

                            if agent_path.exists() {
                                match install_launch_agent(&agent_path) {
                                    Ok(()) => {
                                        s.stop("Menu bar agent installed (starts at login)");
                                    }
                                    Err(e) => {
                                        s.stop(format!(
                                            "Menu bar agent install failed: {e} (non-fatal)"
                                        ));
                                    }
                                }
                            } else {
                                s.stop("Menu bar agent not found (skipped)");
                                println!(
                                    "{} locald-agent not found at {}. The menu bar agent won't start at login.",
                                    style::WARN,
                                    agent_path.display()
                                );
                            }
                        }

                        cliclack::outro("Setup complete")?;
                        println!("Next: run `locald up`.");
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
                        }

                        {
                            let s = cliclack::spinner();
                            s.start("Removing port forwarding rules...");
                            match crate::port_forward::macos::remove() {
                                Ok(()) => {
                                    s.stop("Port forwarding rules removed");
                                    println!(
                                        "{} Removed pfctl redirect rules (com.locald/redirect).",
                                        style::CHECK
                                    );
                                }
                                Err(e) => {
                                    s.error(format!("Port forwarding removal failed: {e}"));
                                    return Err(CliError::message(format!(
                                        "Port forwarding teardown failed: {e}"
                                    )));
                                }
                            }
                        }

                        // Reset privileged_ports config to default.
                        {
                            let s = cliclack::spinner();
                            s.start("Resetting configuration...");
                            let mut config = crate::global_config::load();
                            config.server.privileged_ports = false;
                            match crate::global_config::save(config) {
                                Ok(()) => s.stop("Configuration reset (privileged_ports = false)"),
                                Err(e) => s.stop(format!("Config reset failed: {e} (non-fatal)")),
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
                    // Fetch services
                    let IpcResponse::Status(services) = client::send_request(&IpcRequest::Status)?
                    else {
                        return Err(CliError::message("Failed to get status from daemon"));
                    };

                    let domains: HashSet<String> =
                        services.into_iter().filter_map(|s| s.domain).collect();

                    let mut domain_list: Vec<String> = domains.into_iter().collect();
                    domain_list.sort();

                    #[cfg(unix)]
                    if !nix::unistd::geteuid().is_root() {
                        // Check if we are already running under shim
                        if std::env::var("LOCALD_SHIM_ACTIVE").is_ok() {
                            return Err(CliError::message(
                                "Failed to elevate privileges via shim (still not root).",
                            ));
                        }

                        // Try to escalate via shim
                        if let Ok(Some(shim_path)) = locald_utils::shim::find_privileged() {
                            // Exec shim
                            use std::os::unix::process::CommandExt;
                            let err = std::process::Command::new(&shim_path)
                                .arg("admin")
                                .arg("sync-hosts")
                                .args(&domain_list)
                                .exec();
                            eprintln!("Failed to exec shim: {err}");
                        }

                        return Err(CliError::message(
                            "This command requires root privileges. Please run with sudo or ensure locald-shim is configured.",
                        ));
                    }

                    println!("Syncing {} domains to hosts file...", domain_list.len());

                    let hosts = HostsFileSection::new();
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;

                    let content = rt
                        .block_on(hosts.read())
                        .context("Failed to read hosts file")?;
                    let new_content = hosts.update_content(&content, &domain_list);
                    rt.block_on(hosts.write(&new_content))
                        .context("Failed to write hosts file")?;

                    println!("Hosts file updated.");
                }
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
                        "privileged_ports = {}  (from {})",
                        loader.global.server.privileged_ports,
                        loader.explain_global("server.privileged_ports")
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
                    Ok(r) => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
                    Err(e) => return Err(e),
                }
            }
            RegistryCommands::Pin { path } => {
                utils::ensure_daemon_running()?;
                let abs_path = std::fs::canonicalize(path).context("Failed to resolve path")?;
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
                let abs_path = std::fs::canonicalize(path).context("Failed to resolve path")?;
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
                        println!("{} Removed {} non-existent projects.", style::CHECK, count);
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
fn install_launch_agent(agent_path: &std::path::Path) -> anyhow::Result<()> {
    let label = "com.locald.agent";

    // When running under sudo, resolve the real user's home for the plist location.
    let user_home = if nix::unistd::geteuid().is_root() {
        if let Ok(sudo_user) = std::env::var("SUDO_USER")
            && let Ok(Some(user)) = nix::unistd::User::from_name(&sudo_user)
        {
            Some(user.dir)
        } else {
            None
        }
    } else {
        None
    };

    // Write the plist directly to the correct user's LaunchAgents directory.
    let plist_dir = if let Some(ref home) = user_home {
        home.join("Library/LaunchAgents")
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
            .join("Library/LaunchAgents")
    };
    std::fs::create_dir_all(&plist_dir)?;

    let plist_path = plist_dir.join(format!("{}.plist", label));

    // Write a minimal launchd plist.
    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>"#,
        label = label,
        program = agent_path.display(),
    );
    std::fs::write(&plist_path, plist_content)?;

    // Unload any existing agent, then load the new plist.
    #[allow(clippy::disallowed_methods)]
    let _ = std::process::Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(&plist_path)
        .output();

    #[allow(clippy::disallowed_methods)]
    let status = std::process::Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&plist_path)
        .status()
        .context("Failed to run launchctl load")?;

    if !status.success() {
        anyhow::bail!("launchctl load failed with status: {status}");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launch_agent() -> anyhow::Result<()> {
    let label = "com.locald.agent";
    let plist_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"));

    if plist_path.exists() {
        #[allow(clippy::disallowed_methods)]
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&plist_path)
            .output();
        std::fs::remove_file(&plist_path)?;
    }

    Ok(())
}
