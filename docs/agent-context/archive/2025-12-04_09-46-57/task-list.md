# Phase 23 (Part 2): Foreman Parity

**Goal**: Achieve feature parity with Foreman/Heroku for local development workflows by supporting `.env` files and custom signal handling.

## Tasks

- [x] **.env Support**: Load environment variables from `.env` file in the project root.
- [x] **Signal Handling**: Allow configuring the stop signal (e.g., `SIGINT`, `SIGQUIT`) per service.
- [x] **Verification**: Verify that `.env` vars are loaded and signals are respected.
