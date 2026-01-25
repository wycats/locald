---
title: Testing Philosophy
stage: 3
---

# RFC 0082: Testing Philosophy: No Mocks, Just Fakes

## 1. Context

As `locald` grows in complexity (managing processes, containers, networks, and file systems), ensuring correctness becomes critical. Traditional testing approaches often rely on "Mocking" (hijacking imports or functions to return canned responses). This leads to brittle tests that are coupled to implementation details and fail to catch integration bugs.

We need a testing strategy that promotes:

1.  **Refactoring Safety**: Tests should verify _behavior_, not implementation.
2.  **Speed**: Tests should run fast (in-memory) where possible.
3.  **Realism**: Tests should exercise the actual logic paths, not just mock them out.

## 2. The Philosophy: No Mocks, Just Fakes

We adopt the "No Mocks, Just Fakes" philosophy.

- **No Mocks**: We do not use mocking libraries to patch functions or modules at runtime.
- **Dependency Injection (DI)**: We explicitly pass dependencies (FileSystem, ProcessRunner, Network) to our components.
- **Fakes**: We implement "Fake" versions of these dependencies that behave like the real thing but run in memory.

### 2.1. Rust Implementation (Traits & Structs)

In Rust, this is achieved via **Traits**.

#### Step 1: Define the Interface

Instead of calling `std::fs::write` directly, we define a trait:

```rust
#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
}
```

#### Step 2: Implement "Real" and "Fake"

**Real (Production):**

```rust
pub struct RealFileSystem;

#[async_trait]
impl FileSystem for RealFileSystem {
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        tokio::fs::write(path, content).await.map_err(Into::into)
    }
    // ...
}
```

**Fake (Test):**

```rust
pub struct FakeFileSystem {
    files: RwLock<HashMap<PathBuf, Vec<u8>>>,
}

#[async_trait]
impl FileSystem for FakeFileSystem {
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.files.write().await.insert(path.to_path_buf(), content.to_vec());
        Ok(())
    }
    // ...
}
```

#### Step 3: Inject

```rust
pub struct ConfigManager<FS: FileSystem> {
    fs: FS,
}

impl<FS: FileSystem> ConfigManager<FS> {
    pub fn new(fs: FS) -> Self {
        Self { fs }
    }

    pub async fn save(&self) -> Result<()> {
        self.fs.write(Path::new("config.toml"), b"data").await
    }
}
```

#### Step 4: Test

```rust
#[tokio::test]
async fn test_save_config() {
    let fs = FakeFileSystem::new();
    let manager = ConfigManager::new(fs.clone());

    manager.save().await.unwrap();

    assert_eq!(fs.read(Path::new("config.toml")).await.unwrap(), b"data");
}
```

### 2.2. Frontend Implementation (Svelte & DI)

For the Dashboard (Svelte), we use a similar approach.

- **Interfaces**: Define TypeScript interfaces for API clients.
- **Context/Props**: Pass the client implementation via Svelte Context or Props.
- **Fakes**: Implement in-memory versions of the API client for tests.

## 3. Alignment with Axioms

This philosophy is not arbitrary; it is derived directly from our core design axioms.

### 3.1. Axiom 12: The Source of Truth

**"We discover, we don't invent."**

Our tests must respect this. We should not "invent" a configuration in a mock that cannot exist in reality.

- **Implication**: Fakes must enforce the same constraints as the real system (e.g., a FakeFileSystem should reject invalid paths just like the OS would).
- **Implication**: E2E tests should use real `locald.toml` files and real project structures, not synthetic internal state injection.

### 3.2. Axiom 13: The Development Loop

**"The Build is Isolated; The Runtime is Interactive."**

Our testing strategy mirrors this split:

- **Unit/Integration (Build-like)**: Isolated, hermetic, deterministic. Fakes ensure that external noise (network, OS) doesn't affect the logic verification.
- **E2E (Runtime-like)**: Interactive, permeable. The `locald-e2e` harness runs the real binary in a real (sandboxed) environment, verifying that the "interactive" parts (sockets, signals, PTYs) actually work.

### 3.3. Axiom 7: Ephemeral Runtime, Persistent Context

**"The runtime state of a service is ephemeral, but the context surrounding it is persistent."**

- **Implication**: Tests must verify persistence. It is not enough to check that a service starts; we must check that if we kill it and restart the daemon, the _context_ (logs, status) is preserved.
- **Implication**: Fakes for persistence layers (e.g., `FakeStateStore`) must actually persist data in memory across "restarts" of the logic under test.

## 4. E2E Testing Strategy

While "Fakes" are great for unit and integration logic, we still need **End-to-End (E2E)** tests to verify the actual binary works in the real world.

### 3.1. The `locald-e2e` Crate

We have established `locald-e2e` as the home for black-box testing.

- **Real Binary**: It runs the actual compiled `locald` binary.
- **Sandboxing**: It uses the `--sandbox` flag to isolate the test environment (filesystem, sockets).
- **No Mocks**: It does _not_ mock internal internals. It interacts with the daemon via the CLI and IPC socket, just like a user.

### 3.2. When to use what?

| Test Type       | Scope                  | Dependencies      | Speed   | Purpose                                                     |
| :-------------- | :--------------------- | :---------------- | :------ | :---------------------------------------------------------- |
| **Unit**        | Single Function/Struct | Fakes (In-Memory) | Instant | Verify logic, edge cases, state transitions.                |
| **Integration** | Module/Component       | Fakes (In-Memory) | Fast    | Verify components work together (e.g., Manager + Registry). |
| **E2E**         | Full System            | Real (Sandboxed)  | Slow    | Verify the binary boots, binds ports, and handles commands. |

## 4. Implementation Plan

1.  **Refactor Core**: Identify hardcoded dependencies in `locald-server` (e.g., `std::process::Command`, `tokio::fs`) and extract them into Traits in `locald-core` or `locald-utils`.
2.  **Create Fakes**: Implement `FakeProcessRunner`, `FakeFileSystem`, `FakeNetwork` in a `locald-test-support` crate.
3.  **Migrate Tests**: Refactor existing unit tests to use these Fakes instead of temporary files or real processes where appropriate.
4.  **Expand E2E**: Continue adding scenarios to `locald-e2e` for critical user flows (Start, Stop, Logs).

## 5. Benefits

- **Determinism**: Fakes don't flake due to OS timing or network issues.
- **Speed**: Running thousands of tests in memory is orders of magnitude faster than spawning processes.
- **Safety**: We can simulate error conditions (disk full, network down) that are hard to reproduce with real dependencies.
