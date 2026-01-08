# Toolbx / Distrobox / Container Development

`locald` is designed to work in **containerized development environments** like [Toolbx](https://containertoolbx.org/) and [Distrobox](https://distrobox.it/).

## The Canonical Workflow

```
┌─────────────────────────────────────────────────────────────────┐
│ HOST OS (Fedora Silverblue, etc.)                               │
│                                                                 │
│   1. Install locald:                                            │
│      cargo install locald                                       │
│                                                                 │
│   2. Run privileged setup (once):                               │
│      sudo locald admin setup                                    │
│                                                                 │
│   This installs the setuid shim and configures cgroups.         │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ (shared home directory)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ TOOLBX / DISTROBOX CONTAINER                                    │
│                                                                 │
│   3. Run your services:                                         │
│      cd ~/Code/my-project                                       │
│      locald up                                                  │
│                                                                 │
│   Your locald.toml scripts run in the container environment     │
│   (with your dev tools, language runtimes, etc.)                │
└─────────────────────────────────────────────────────────────────┘
```

### Why This Split?

| Concern                                                  | Where     |
| -------------------------------------------------------- | --------- |
| **Privileged setup** (cgroups, setuid shim, HTTPS trust) | Host OS   |
| **Development environment** (compilers, runtimes, tools) | Container |
| **Service execution** (your `locald.toml` commands)      | Container |

The setuid shim installed on the host cannot be used directly from inside the container (user namespace mapping makes it appear as `nobody:nobody`). This is expected and handled gracefully.

## Behavior Inside Containers

When you run `locald up` inside Toolbx/Distrobox:

1. `locald` detects the container environment (via `$container`, `/run/.containerenv`, etc.)
2. It warns that privileged features are unavailable:
   ```
   ⚠ locald-shim is not available as a privileged helper in this container.
   ⚠ Continuing without privileged features (hosts sync, cgroup isolation, privileged ports).
   ⚠ For full setup, run `sudo locald admin setup` on the host OS.
   ```
3. It proceeds to run your services normally

### What Works Without Privileged Features

| Feature                             | Status   |
| ----------------------------------- | -------- |
| Running services from `locald.toml` | ✅ Works |
| Port discovery and assignment       | ✅ Works |
| Service dependencies                | ✅ Works |
| Dashboard                           | ✅ Works |
| Logs and monitoring                 | ✅ Works |

### What Requires Host Setup

| Feature                         | Requires Host Setup |
| ------------------------------- | ------------------- |
| Privileged ports (80, 443)      | Yes                 |
| `/etc/hosts` auto-sync          | Yes                 |
| cgroup-based process isolation  | Yes                 |
| HTTPS with system-trusted certs | Yes                 |

## Alternative: `--host` Flag

If you have `flatpak-spawn` available (common on Flatpak-based systems), you can run setup from inside the container:

```bash
# Inside Toolbx/Distrobox:
locald admin setup --host
```

This uses `flatpak-spawn --host` or `distrobox-host-exec` to run setup on the host OS.

## Troubleshooting

### "locald-shim is not available as a privileged helper"

This is expected inside containers. Run `sudo locald admin setup` on the host OS.

### Setup ran but services fail with permission errors

Make sure you ran setup on the **host**, not inside the container. The shim must be setuid root on the host filesystem.

### Services can't bind to port 80/443

Privileged port binding requires the shim, which isn't available inside containers. Use high ports (e.g., 8080) or run setup on the host.

## Testing This Workflow

The `locald-e2e` test suite includes container-aware tests. To verify the workflow:

```bash
# On host: ensure shim is installed
sudo locald admin setup

# Inside container: run locald
toolbox run locald doctor
toolbox run locald up
```

The `locald doctor` command reports privileged feature availability.
