# Locald Container Engine Specification

This document defines the **implicit requirements** and **responsibilities** that `locald` assumes when acting as a Container Engine. These are the actions required to turn a static OCI RootFS into a functioning Linux environment.

## 1. Identity & User Resolution

**Requirement**: The container environment MUST allow resolving User IDs (UIDs) and Group IDs (GIDs) to names, and vice-versa.

- **Provenance**: [POSIX.1-2017 (System Interfaces)](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/pwd.h.html).
  - **Strictly Speaking**: POSIX mandates a "User Database" accessible via `<pwd.h>` APIs (like `getpwnam`). It does not strictly mandate a flat file at `/etc/passwd`.
  - **In Practice**: In a container environment (which lacks external directory services like LDAP or NIS), the `/etc/passwd` file is the standard implementation of this database. Without it, standard POSIX tools (`ls`, `tar`, `ps`) cannot map UIDs to names.
- **Context**: In a "rootless" container using User Namespaces, the container's view of users (UID 0 = root) differs from the host. Without explicit mapping files, tools running as "root" (UID 0) inside the container may fail to recognize themselves or file ownership.

**Specification**:
If the RootFS does not contain valid user databases, `locald` MUST synthesize them.

- **`/etc/passwd`**:
  - Must contain an entry for `root` (UID 0).
  - Must contain an entry for the default user (e.g., `cnb` UID 1000).
- **`/etc/group`**:
  - Must contain matching groups.

## 2. Network & DNS

**Requirement**: The container MUST be able to resolve DNS queries to access the network (if network access is permitted).

- **Provenance**: **De-facto Standard (Docker/Podman)**. The OCI Image Spec explicitly excludes configuration that depends on the runtime environment. DNS is a property of the _network_, not the _image_.
- **Context**: `/etc/resolv.conf` inside a static image is usually empty or invalid for the runtime network.

**Specification**:
`locald` MUST inject DNS configuration at runtime.

- **Action**: Copy the host's `/etc/resolv.conf` to the container's `/etc/resolv.conf` (or bind-mount it).
- **Fallback**: If the host's configuration is incompatible (e.g., points to a local resolver like `127.0.0.53` that is unreachable from the container), `locald` SHOULD provide a safe fallback (e.g., Google DNS `8.8.8.8`).

## 3. Kernel Filesystems

**Requirement**: The container MUST have access to standard kernel interfaces.

- **Provenance**: **Linux Kernel / FHS**.
- **Context**: Processes expect to interact with the kernel via specific mount points.

**Specification**:
`locald` MUST mount the following filesystems:

| Path       | Type     | Options                           | Provenance                                                       |
| :--------- | :------- | :-------------------------------- | :--------------------------------------------------------------- |
| `/proc`    | `proc`   | `nosuid`, `noexec`, `nodev`       | Required for process management (`ps`, `top`) and introspection. |
| `/sys`     | `sysfs`  | `nosuid`, `noexec`, `nodev`, `ro` | Required for hardware/kernel info. Usually read-only.            |
| `/dev`     | `tmpfs`  | `nosuid`, `mode=755`              | Populated with devices.                                          |
| `/dev/pts` | `devpts` | `nosuid`, `noexec`, `newinstance` | Required for PTY/TTY support (interactive terminals).            |
| `/dev/shm` | `tmpfs`  | `nosuid`, `noexec`, `nodev`       | Required for POSIX shared memory.                                |

## 4. Essential Devices

**Requirement**: Standard I/O devices MUST be present in `/dev`.

- **Provenance**: **Linux Standard Base (LSB)**.

**Specification**:
`locald` MUST create (or bind-mount) the following devices in `/dev`:

- `/dev/null`
- `/dev/zero`
- `/dev/full`
- `/dev/random`
- `/dev/urandom`
- `/dev/tty`
- `/dev/console`

## 5. Temporary Directories

**Requirement**: A writable temporary directory MUST exist.

- **Provenance**: **POSIX**.

**Specification**:

- `/tmp` MUST be a writable directory (usually a `tmpfs`) with mode `1777` (sticky bit).

## 6. Environment Variables

**Requirement**: Processes MUST inherit a minimal execution environment.

- **Provenance**: **POSIX / Convention**.

**Specification**:
Unless overridden, `locald` SHOULD provide:

- `PATH`: A standard path (e.g., `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`).
- `HOME`: The home directory of the running user.
- `TERM`: `xterm` or similar (if a TTY is allocated).

## Prior Art & References

The responsibilities defined above are derived from the behavior of the de-facto standard container engine, **Moby (Docker)**. While the OCI Runtime Spec defines the _mechanism_ for configuration, it does not mandate the _content_ of that configuration. The "Engine" layer fills this gap.

### 1. DNS Configuration (`/etc/resolv.conf`)

Docker explicitly manages `resolv.conf` to ensure containers have network access. It filters out localhost addresses (which would be invalid inside the container) and injects public DNS (like Google's `8.8.8.8`) if no valid upstream resolvers are found.

- **Reference**: `moby/daemon/libnetwork/resolvconf/resolvconf.go`
- **Behavior**: Parses host `resolv.conf`, transforms it (removing loopback), and mounts it into the container.

### 2. Identity (`/etc/passwd`)

While Docker typically expects the container image to provide `/etc/passwd`, it parses this file to resolve `USER` directives. In the context of **Cloud Native Buildpacks (CNB)**, the `lifecycle` binary explicitly requires a valid user environment. When operating in a synthetic or scratch environment (like our builder), the engine must ensure these files exist to satisfy POSIX expectations of the tools running inside.

- **Reference**: `moby/daemon/oci_linux.go` (`getUser` function)
- **Behavior**: Resolves username to UID/GID by reading the container's `/etc/passwd`.

### 3. OCI Whiteouts

Handling `.wh.` files is a strict requirement of the OCI Image Layer Specification.

- **Reference**: [OCI Image Layer Filesystem Changeset](https://github.com/opencontainers/image-spec/blob/main/layer.md#whiteouts)

### 4. Hostname & Hosts File (`/etc/hosts`)

Docker manages `/etc/hosts` to ensure that the container can resolve its own hostname to a loopback address, and to inject user-defined host mappings.

- **Reference**: `moby/daemon/libnetwork/sandbox_dns_unix.go` (`buildHostsFile` function)
- **Behavior**: Creates a hosts file containing `127.0.0.1 localhost` and `127.0.0.1 <hostname>`.

## Philosophy: The Implicit Stack

The "Container Spec" ecosystem is fragmented into layers. The "Engine" layer (occupied by Docker/Podman and now `locald`) is largely defined by **convention** and **POSIX requirements**, not by the OCI specs themselves.

1.  **OCI Runtime Spec (runc)**: Defines _mechanism_ (how to mount a file), but not _policy_ (what files to mount). It assumes the caller knows the requirements.
2.  **OCI Image Spec**: Defines the static artifact. It explicitly excludes runtime configuration like DNS.
3.  **POSIX / Linux Standard Base**: The source of the actual requirements. Tools like `pip`, `apt`, and `lifecycle` expect a POSIX environment (`/etc/passwd`, `/etc/resolv.conf`, `/tmp`, `/dev/null`).
4.  **The "De Facto" Spec (Moby/Docker)**: Docker fills the gap between OCI and POSIX. It has specific code to inject these files because the OCI spec doesn't mandate them.

## Implementation Compliance Matrix

The following table maps `locald`'s implementation against the reference implementation in Moby (Docker). This confirms that `locald` is implementing standard Container Engine responsibilities, not ad-hoc fixes.

| Feature       | Moby Implementation                                                         | Locald Implementation                                                           | Status                              |
| :------------ | :-------------------------------------------------------------------------- | :------------------------------------------------------------------------------ | :---------------------------------- |
| **DNS**       | `resolvconf.go`: Copies host file, strips localhost, adds 8.8.8.8 fallback. | `builder.rs`: Copies host file, adds 8.8.8.8 fallback.                          | **MATCH**                           |
| **Identity**  | `oci_linux.go`: Parses `/etc/passwd` from image.                            | `builder.rs`: Synthesizes `/etc/passwd` (required for scratch/synthetic roots). | **MATCH** (Functionally equivalent) |
| **Hosts**     | `sandbox_dns_unix.go`: Creates `/etc/hosts` with `localhost` and hostname.  | **Missing** (Not currently breaking builds).                                    | **GAP** (Minor)                     |
| **Mounts**    | `oci_linux.go`: Mounts `/proc`, `/sys`, `/dev`, `/dev/pts`, `/dev/shm`.     | `builder.rs`: Relies on `runc` spec generation + directory creation.            | **MATCH**                           |
| **Whiteouts** | `layer/layer.go`: Skips `.wh.` files.                                       | `builder.rs`: Skips `.wh.` files.                                               | **MATCH**                           |

**Conclusion**: `locald` implements the **critical path** for a functioning build environment. The "hacks" (synthesizing files, injecting DNS) are, in fact, the formal responsibilities of any OCI-compliant Container Engine.
