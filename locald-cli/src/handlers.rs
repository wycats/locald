use anyhow::{Context, Result};
use crossterm::style::Stylize;
use locald_core::{HostsFileSection, IpcRequest, IpcResponse, LocaldConfig};
use std::collections::HashSet;

use crate::cli::{
    AddServiceType, AdminCommands, AiCommands, Cli, Commands, ConfigCommands, DebugCommands,
    RegistryCommands, ServerCommands, ServiceCommands,
};
use crate::{
    build, client, container, debug, history, init, monitor, run, service, style, trust, try_cmd,
    utils,
};

pub fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Commands::Init => {
            init::run()?;
        }
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
            let cmd_str = if command.len() == 1 && command[0] == "last" {
                history::get_last().context("No history found")?
            } else {
                command.join(" ")
            };
            service::add_exec(cmd_str, name.clone(), *port)?;
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
                        eprintln!("{} Failed to reset {full_name}: {msg}", style::CROSS);
                    }
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
                }
            }
        },
        Commands::Monitor => {
            utils::ensure_daemon_running()?;
            monitor::run()?;
        }
        Commands::Ping => match client::send_request(&IpcRequest::Ping) {
            Ok(response) => println!("Received: {response:?}"),
            Err(e) => utils::handle_ipc_error(&e),
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
                Err(e) => utils::handle_ipc_error(&e),
            },
            ServerCommands::Restart => {
                match client::send_request(&IpcRequest::Shutdown) {
                    Ok(_) => println!("Shutting down locald..."),
                    Err(e) => {
                        if !e.to_string().contains("locald is not running") {
                            utils::handle_ipc_error(&e);
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
        Commands::Up { path, verbose } => {
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
                    if e.to_string().contains("locald is not running") {
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
                                utils::handle_ipc_error(&e);
                                break;
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
        }
        Commands::Stop { name } => {
            let names = if let Some(n) = name {
                vec![n.clone()]
            } else {
                let config_path = std::env::current_dir()?.join("locald.toml");
                if !config_path.exists() {
                    anyhow::bail!(
                        "No locald.toml found in current directory. Please specify a service name."
                    );
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

            for service_name in names {
                match client::send_request(&IpcRequest::Stop {
                    name: service_name.clone(),
                }) {
                    Ok(IpcResponse::Ok) => {
                        println!("{} Stopped service {}", style::CHECK, service_name.bold());
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        eprintln!("{} Failed to stop {service_name}: {msg}", style::CROSS);
                    }
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
                }
            }
        }
        Commands::Restart { name } => {
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
                    println!("{} Restarted service {}", style::CHECK, full_name.bold());
                }
                Ok(IpcResponse::Error(msg)) => {
                    eprintln!("{} Failed to restart {full_name}: {msg}", style::CROSS);
                }
                Ok(r) => println!("Unexpected response: {r:?}"),
                Err(e) => utils::handle_ipc_error(&e),
            }
        }
        Commands::Status => {
            utils::ensure_daemon_running()?;
            match client::send_request(&IpcRequest::Status) {
                Ok(IpcResponse::Status(services)) => {
                    if services.is_empty() {
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
                        }
                    }
                }
                Ok(response) => println!("Unexpected response: {response:?}"),
                Err(e) => utils::handle_ipc_error(&e),
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

            if let Err(e) = client::stream_logs(service_name, *follow) {
                utils::handle_ipc_error(&e);
            }
        }
        Commands::Admin { command } => {
            match command {
                AdminCommands::Setup => {
                    const SHIM_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_SHIM_PATH"));

                    #[cfg(unix)]
                    if !nix::unistd::geteuid().is_root() {
                        anyhow::bail!(
                            "This command requires root privileges. Please run with sudo."
                        );
                    }

                    #[cfg(target_os = "linux")]
                    {
                        let exe_path = std::env::current_exe()?;
                        let exe_dir = exe_path.parent().context("Failed to get exe directory")?;
                        let shim_path = exe_dir.join("locald-shim");

                        println!("Installing locald-shim to {}", shim_path.display());
                        locald_utils::shim::install(&shim_path, SHIM_BYTES)?;

                        println!("locald-shim installed and configured.");
                        println!("Next: run `locald up`.");
                    }

                    #[cfg(not(target_os = "linux"))]
                    {
                        anyhow::bail!("Admin setup is only supported on Linux.");
                    }

                    // Note: We don't setcap on locald anymore, because the shim handles it.
                    // But if the user runs locald directly without shim, it won't have caps.
                    // That's fine, the shim is the intended way for privileged ops.
                }
                AdminCommands::SyncHosts => {
                    // Fetch services
                    let IpcResponse::Status(services) = client::send_request(&IpcRequest::Status)?
                    else {
                        anyhow::bail!("Failed to get status from daemon");
                    };

                    let domains: HashSet<String> =
                        services.into_iter().filter_map(|s| s.domain).collect();

                    let mut domain_list: Vec<String> = domains.into_iter().collect();
                    domain_list.sort();

                    #[cfg(unix)]
                    if !nix::unistd::geteuid().is_root() {
                        // Check if we are already running under shim
                        if std::env::var("LOCALD_SHIM_ACTIVE").is_ok() {
                            anyhow::bail!(
                                "Failed to elevate privileges via shim (still not root)."
                            );
                        }

                        // Try to escalate via shim
                        if let Ok(Some(shim_path)) = locald_utils::shim::find() {
                            // Check if shim is setuid
                            if let Ok(metadata) = std::fs::metadata(&shim_path) {
                                use std::os::unix::fs::MetadataExt;
                                if (metadata.mode() & 0o4000) != 0 {
                                    // Exec shim
                                    use std::os::unix::process::CommandExt;
                                    let err = std::process::Command::new(&shim_path)
                                        .arg("admin")
                                        .arg("sync-hosts")
                                        .args(&domain_list)
                                        .exec();
                                    eprintln!("Failed to exec shim: {err}");
                                }
                            }
                        }

                        anyhow::bail!(
                            "This command requires root privileges. Please run with sudo or ensure locald-shim is configured."
                        );
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
            AiCommands::Schema => match client::send_request(&IpcRequest::AiSchema) {
                Ok(IpcResponse::AiSchema(schema)) => println!("{schema}"),
                Ok(r) => println!("Unexpected response: {r:?}"),
                Err(e) => utils::handle_ipc_error(&e),
            },
            AiCommands::Context => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::AiContext) {
                    Ok(IpcResponse::AiContext(context)) => println!("{context}"),
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
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
                    println!("Global Configuration:");
                    println!("  Path: {:?}", loader.global_path);
                    println!("  Values: {:#?}", loader.global);
                } else {
                    println!("{}", toml::to_string_pretty(&loader.global)?);
                }
            }
        },
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
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
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
                        eprintln!("{} Failed to pin project: {msg}", style::CROSS);
                    }
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
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
                        eprintln!("{} Failed to unpin project: {msg}", style::CROSS);
                    }
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
                }
            }
            RegistryCommands::Clean => {
                utils::ensure_daemon_running()?;
                match client::send_request(&IpcRequest::RegistryClean) {
                    Ok(IpcResponse::RegistryCleaned(count)) => {
                        println!("{} Removed {} non-existent projects.", style::CHECK, count);
                    }
                    Ok(IpcResponse::Error(msg)) => {
                        eprintln!("{} Failed to clean registry: {msg}", style::CROSS);
                    }
                    Ok(r) => println!("Unexpected response: {r:?}"),
                    Err(e) => utils::handle_ipc_error(&e),
                }
            }
        },
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
        Commands::Serve {
            path,
            port,
            bind: _,
        } => {
            let abs_path = std::fs::canonicalize(path).context("Failed to resolve path")?;
            if !abs_path.exists() {
                anyhow::bail!("Path does not exist: {}", abs_path.display());
            }
            if !abs_path.is_dir() {
                anyhow::bail!("Path is not a directory: {}", abs_path.display());
            }

            // Run the static server
            // We use a blocking call here because the CLI command is long-running
            let (tx, _) = tokio::sync::broadcast::channel(100);

            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(locald_server::static_server::run_static_server(
                    *port, abs_path, tx,
                ))?;
        }
    }

    Ok(())
}
