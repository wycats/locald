use oci_spec::runtime::{
    LinuxIdMappingBuilder, LinuxNamespaceBuilder, LinuxNamespaceType, MountBuilder, ProcessBuilder,
    RootBuilder, Spec, SpecBuilder,
};
use std::path::Path;

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

    println!(
        "DEBUG: Generating config with {} namespaces",
        namespaces.len()
    );

    let linux = oci_spec::runtime::LinuxBuilder::default()
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
        ])
        .build()?;

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
