# Fresh Eyes Review: Docker Integration

**Reviewer**: Thinking Partner
**Date**: 2025-11-30
**Subject**: `docs/design/docker-integration.md`

## Summary
The proposal aims to integrate Docker container management directly into `locald`, allowing users to define infrastructure dependencies (like databases) in `locald.toml`. The recommendation is "Approach 1: Native Docker Support," where `locald` wraps the `docker` CLI.

## Critique

### 1. Alignment with Personas

*   **App Builder ("Regular Joe")**:
    *   **Pros**: This is a huge win. Defining `image = "postgres"` and having a database appear is exactly the "magic" they want. It removes the cognitive load of `docker-compose.yml` syntax, networking, and port mapping.
    *   **Cons**: If they *already* have a `docker-compose.yml` (common in existing projects), they might feel forced to migrate or maintain two configs.
*   **Power User ("System Tweaker")**:
    *   **Pros**: They get dynamic port assignment for containers, which `docker-compose` makes hard.
    *   **Cons**: They will immediately hit the limits of the abstraction. "How do I mount a custom config file?", "How do I set a network alias?", "How do I use a custom entrypoint?". Re-implementing the full surface area of `docker run` in `locald.toml` is a slippery slope.

### 2. Alignment with Axioms

*   **Axiom 4: Process Ownership**:
    *   The proposal aligns well. `locald` owns the `docker` CLI process, which in turn owns the container (via `--rm` and signal forwarding).
    *   **Risk**: `docker` client process is just a client. The actual container runs in the Docker daemon. If `locald` crashes, the `docker` CLI process dies, but does the container die?
        *   *Mitigation*: `docker run --rm` usually handles this if the client disconnects, but it's not guaranteed. We might need a "reaper" logic on startup to clean up tagged containers.
*   **Axiom 3: Managed Ports**:
    *   The proposal to use dynamic ports (`0:5432`) is excellent. It solves the "Port 5432 already in use" problem that plagues `docker-compose` users.

### 3. Technical Feasibility & Dragons

*   **The "localhost" Trap**:
    *   If Service A (local binary) talks to Service B (Docker Postgres), Service A connects to `localhost:RANDOM_PORT`. This works fine.
    *   If Service B (Docker) needs to talk to Service A (local binary), `localhost` inside the container refers to the container itself.
    *   *Solution*: This is a known Docker issue. On Linux, `--network host` works but isolates ports less. On Mac/Windows, `host.docker.internal` is needed. `locald` might need to inject a special env var like `HOST_URL` that resolves correctly depending on the OS.
*   **Volume Permissions**:
    *   Mounting `./data:/var/lib/postgresql/data` often leads to permission issues on Linux because the container runs as root/postgres and the host dir is owned by the user.
    *   *Mitigation*: This is out of scope for `locald` to fix entirely, but we should document it or provide reasonable defaults.
*   **Zombie Containers**:
    *   If `locald` is `kill -9`'d, the `docker run` process dies. Does Docker kill the container?
    *   *Test required*: We need to verify if `--rm` is sufficient or if we need to explicitly name containers (`locald-<project>-<service>`) and kill them on startup (Axiom 4 implies we clean up before starting).

### 4. The "Re-inventing Docker Compose" Risk

The proposal dismisses Approach 2 (Compose Delegation) too quickly.
*   **Counter-argument**: `docker-compose` is the industry standard.
*   **Hybrid Proposal**:
    *   Support `image` for simple cases (90% of App Builder needs).
    *   Support `docker_args` (raw list of strings) for Power Users who need weird flags.
    *   *Don't* try to support every Docker feature (networks, complex volumes, healthchecks) in the schema. If they need that, they should use `command = "docker-compose up"` (which we already support as a raw command!).

## Recommendations

1.  **Adopt Approach 1 (Native)** but keep the schema **strict and minimal**.
    *   `image` (required)
    *   `container_port` (required for port mapping)
    *   `env` (standard)
    *   `volumes` (list of strings, passed directly to `-v`)
    *   **Do not** add `network`, `user`, `entrypoint`, etc. yet.
2.  **Robust Cleanup**:
    *   Name containers deterministically: `locald-<project>-<service>`.
    *   On `locald` startup, run `docker rm -f locald-<project>-<service>` to ensure a clean slate (Axiom 4).
3.  **Networking Helper**:
    *   Investigate injecting `host.docker.internal` or equivalent into the container's `/etc/hosts` or environment if possible, to allow container-to-host communication.

## Verdict
**Proceed with Approach 1**, but prioritize **robust cleanup** and **minimal schema** over feature parity with Docker Compose.
