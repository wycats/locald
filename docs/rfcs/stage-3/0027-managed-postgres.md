<!-- exo:27 ulid:01krkpxdvnq2kz0qp65vt622sj -->

# RFC 27: Managed Postgres: postgresql_embedded


# RFC: Managed Postgres: postgresql_embedded

## 1. Summary

Use `postgresql_embedded` to provide zero-config Postgres instances.

## 2. Motivation

Users need a database. Installing Postgres manually or via Docker is friction. We want "it just works".

## 3. Detailed Design

Download the binary, init data dir, start on dynamic port.

### Terminology

- **Embedded**: Managed by the application, not the system.

### User Experience (UX)

`locald service add postgres db`.

### Architecture

`PostgresRunner` struct.

### Implementation Details

`postgresql_embedded` crate.

## 4. Drawbacks

- Download size.

## 5. Alternatives

- Docker.
- System Postgres.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Redis, MySQL.
