<!-- exo:28 ulid:01krkpxdvp90gfbe7w9kjpe1vw -->

# RFC 28: Service Configuration: Typed Enum


# RFC: Service Configuration: Typed Enum

## 1. Summary

Refactor `ServiceConfig` to use a Serde tagged enum (`type` field).

## 2. Motivation

Different services (Exec, Postgres) have different config needs. A single struct with optional fields is messy.

## 3. Detailed Design

```rust
#[serde(tag = "type", rename_all = "snake_case")]
enum ServiceConfig {
    Exec(ExecConfig),
    Postgres(PostgresConfig),
}
```

### Terminology

- **Tagged Enum**: A Rust enum represented as an object with a type tag in JSON/TOML.

### User Experience (UX)

Clearer config validation.

### Architecture

`locald-core`.

### Implementation Details

Serde.

## 4. Drawbacks

- Breaking change for config format (if not careful).

## 5. Alternatives

- Untyped map.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
