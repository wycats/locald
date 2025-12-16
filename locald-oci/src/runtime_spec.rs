use locald_core::config::{ServiceConfig, TypedServiceConfig};
use oci_spec::image::ImageConfiguration;
use oci_spec::runtime::{
    LinuxIdMappingBuilder, LinuxNamespaceBuilder, LinuxNamespaceType, MountBuilder, ProcessBuilder,
    RootBuilder, Spec, SpecBuilder,
};
use std::path::Path;
use tracing::debug;

#[allow(clippy::too_many_arguments)]
#[allow(clippy::similar_names)]
pub fn generate_from_service(
    service_config: &ServiceConfig,
    image_config: &ImageConfiguration,
    rootfs_path: &Path,
    host_uid: u32,
    host_gid: u32,
    container_uid: u32,
    container_gid: u32,
) -> anyhow::Result<Spec> {
    let config = image_config.config().as_ref();

    // 1. Determine Args (Entrypoint + Cmd)
    let mut args = Vec::new();

    if let Some(entrypoint) = config.and_then(|c| c.entrypoint().as_ref()) {
        args.extend(entrypoint.iter().cloned());
    }

    // Check for overrides in ServiceConfig
    let (service_command, service_workdir, service_env) = match service_config {
        ServiceConfig::Typed(TypedServiceConfig::Container(c)) => {
            (c.command.as_ref(), c.workdir.as_ref(), &c.common.env)
        }
        ServiceConfig::Typed(TypedServiceConfig::Exec(c)) => {
            (c.command.as_ref(), c.workdir.as_ref(), &c.common.env)
        }
        _ => (None, None, service_config.env()),
    };

    if let Some(cmd_str) = service_command {
        if let Some(split_args) = shlex::split(cmd_str) {
            args.extend(split_args);
        } else {
            args.push(cmd_str.clone());
        }
    } else if let Some(cmd) = config.and_then(|c| c.cmd().as_ref()) {
        args.extend(cmd.iter().cloned());
    }

    if args.is_empty() {
        args.push("/bin/sh".to_string());
    }

    // 2. Determine Env
    let mut env_map = std::collections::HashMap::new();

    // Image env
    if let Some(image_env) = config.and_then(|c| c.env().as_ref()) {
        for e in image_env {
            if let Some((k, v)) = e.split_once('=') {
                env_map.insert(k.to_string(), v.to_string());
            }
        }
    }

    // Service env overrides
    for (k, v) in service_env {
        env_map.insert(k.clone(), v.clone());
    }

    let env: Vec<String> = env_map.iter().map(|(k, v)| format!("{k}={v}")).collect();

    // 3. Determine Cwd
    let cwd = service_workdir
        .map(std::string::String::as_str)
        .or_else(|| {
            config
                .and_then(|c| c.working_dir().as_ref())
                .map(std::string::String::as_str)
        });

    // 4. Call generate_config
    generate_config(
        rootfs_path,
        &args,
        &env,
        &[], // No bind mounts for now
        host_uid,
        host_gid,
        container_uid,
        container_gid,
        cwd,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::similar_names)]
pub fn generate_spec(
    image_config: &ImageConfiguration,
    rootfs_path: &Path,
    host_uid: u32,
    host_gid: u32,
    container_uid: u32,
    container_gid: u32,
    args_override: Option<&[String]>,
) -> anyhow::Result<Spec> {
    let config = image_config.config().as_ref();

    // 1. Determine Args (Entrypoint + Cmd)
    let mut args = Vec::new();

    if let Some(entrypoint) = config.and_then(|c| c.entrypoint().as_ref()) {
        args.extend(entrypoint.iter().cloned());
    }

    if let Some(override_args) = args_override {
        args.extend(override_args.iter().cloned());
    } else if let Some(cmd) = config.and_then(|c| c.cmd().as_ref()) {
        args.extend(cmd.iter().cloned());
    }

    if args.is_empty() {
        args.push("/bin/sh".to_string());
    }

    // 2. Determine Env
    let env = config
        .and_then(|c| c.env().as_ref())
        .cloned()
        .unwrap_or_default();

    // 3. Determine Cwd
    let cwd = config
        .and_then(|c| c.working_dir().as_ref())
        .map(std::string::String::as_str);

    // 4. Call generate_config
    generate_config(
        rootfs_path,
        &args,
        &env,
        &[], // No bind mounts for now
        host_uid,
        host_gid,
        container_uid,
        container_gid,
        cwd,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::similar_names)]
pub fn generate_config(
    rootfs_path: &Path,
    args: &[String],
    env: &[String],
    bind_mounts: &[(String, String)], // (source, destination)
    uid: u32,
    gid: u32,
    container_uid: u32,
    container_gid: u32,
    cwd: Option<&str>,
    cgroup_path: Option<&str>,
) -> anyhow::Result<Spec> {
    let root = RootBuilder::default()
        .path(rootfs_path)
        .readonly(false)
        .build()?;

    let process = ProcessBuilder::default()
        .args(args)
        .env(env)
        .cwd(cwd.unwrap_or("/workspace"))
        .terminal(false) // For now
        .user(
            oci_spec::runtime::UserBuilder::default()
                .uid(container_uid)
                .gid(container_gid)
                .build()?,
        )
        .build()?;

    // Rootless mapping: Map host user (uid) to container user (container_uid)
    let uid_mapping = LinuxIdMappingBuilder::default()
        .host_id(uid)
        .container_id(container_uid)
        .size(1_u32)
        .build()?;

    let gid_mapping = LinuxIdMappingBuilder::default()
        .host_id(gid)
        .container_id(container_gid)
        .size(1_u32)
        .build()?;

    // Rootless mapping typically requires a user namespace. In that mode, some mounts
    // (notably a bind-mount of host /sys) are commonly blocked by the kernel/LSMs.
    let rootless = uid != 0;

    let namespaces = vec![
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Pid)
            .build()?,
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Ipc)
            .build()?,
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Uts)
            .build()?,
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::Mount)
            .build()?,
        // LinuxNamespaceBuilder::default()
        //     .typ(LinuxNamespaceType::Cgroup)
        //     .build()?,
        LinuxNamespaceBuilder::default()
            .typ(LinuxNamespaceType::User)
            .build()?,
    ];

    debug!(
        "DEBUG: Generating config with {} namespaces",
        namespaces.len()
    );

    let mut linux_builder = oci_spec::runtime::LinuxBuilder::default();
    linux_builder = linux_builder
        .uid_mappings(vec![uid_mapping])
        .gid_mappings(vec![gid_mapping])
        .namespaces(namespaces)
        .masked_paths(vec![
            "/proc/acpi".to_string(),
            "/proc/asound".to_string(),
            "/proc/kcore".to_string(),
            "/proc/keys".to_string(),
            "/proc/latency_stats".to_string(),
            "/proc/timer_list".to_string(),
            "/proc/timer_stats".to_string(),
            "/proc/sched_debug".to_string(),
            "/sys/firmware".to_string(),
            "/proc/scsi".to_string(),
        ])
        .readonly_paths(vec![
            "/proc/bus".to_string(),
            "/proc/fs".to_string(),
            "/proc/irq".to_string(),
            "/proc/sys".to_string(),
            "/proc/sysrq-trigger".to_string(),
        ]);

    if let Some(path) = cgroup_path {
        linux_builder = linux_builder.cgroups_path(std::path::PathBuf::from(path));
    }

    let linux = linux_builder.build()?;

    let mut mounts = vec![
        MountBuilder::default()
            .destination("/proc")
            .typ("proc")
            .source("proc")
            .build()?,
        MountBuilder::default()
            .destination("/dev")
            .typ("tmpfs")
            .source("tmpfs")
            .options(vec![
                "nosuid".to_string(),
                "strictatime".to_string(),
                "mode=755".to_string(),
                "size=65536k".to_string(),
            ])
            .build()?,
        MountBuilder::default()
            .destination("/dev/pts")
            .typ("devpts")
            .source("devpts")
            .options(vec![
                "nosuid".to_string(),
                "noexec".to_string(),
                "newinstance".to_string(),
                "ptmxmode=0666".to_string(),
                "mode=0620".to_string(),
                "gid=0".to_string(),
            ])
            .build()?,
        MountBuilder::default()
            .destination("/tmp")
            .typ("tmpfs")
            .source("tmpfs")
            .options(vec![
                "nosuid".to_string(),
                "noexec".to_string(),
                "nodev".to_string(),
                "mode=1777".to_string(),
            ])
            .build()?,
    ];

    // Only attempt to bind-mount host /sys in non-rootless mode.
    // In rootless containers this is a frequent source of EPERM.
    if !rootless {
        mounts.push(
            MountBuilder::default()
                .destination("/sys")
                .typ("none")
                .source("/sys")
                .options(vec![
                    "rbind".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "ro".to_string(),
                ])
                .build()?,
        );
    }

    for (source, dest) in bind_mounts {
        mounts.push(
            MountBuilder::default()
                .destination(dest)
                .typ("none")
                .source(source)
                .options(vec!["rbind".to_string(), "rw".to_string()])
                .build()?,
        );
    }

    let spec = SpecBuilder::default()
        .version("1.0.2")
        .root(root)
        .process(process)
        .linux(linux)
        .mounts(mounts)
        .build()?;

    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::generate_config;

    fn has_sys_bind_mount(spec: &oci_spec::runtime::Spec) -> bool {
        let Some(mounts) = spec.mounts().as_ref() else {
            return false;
        };

        mounts.iter().any(|m| {
            m.destination().as_os_str() == std::ffi::OsStr::new("/sys")
                && m.source().as_deref() == Some(std::path::Path::new("/sys"))
        })
    }

    #[test]
    fn rootless_spec_omits_sys_bind_mount() {
        let spec = generate_config(
            std::path::Path::new("rootfs"),
            &["/bin/sh".to_string()],
            &[],
            &[],
            1000,
            1000,
            0,
            0,
            None,
            None,
        )
        .expect("spec generation should succeed");

        assert!(!has_sys_bind_mount(&spec));
    }

    #[test]
    fn rootful_spec_includes_sys_bind_mount() {
        let spec = generate_config(
            std::path::Path::new("rootfs"),
            &["/bin/sh".to_string()],
            &[],
            &[],
            0,
            0,
            0,
            0,
            None,
            None,
        )
        .expect("spec generation should succeed");

        assert!(has_sys_bind_mount(&spec));
    }
}
