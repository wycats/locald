# RFC 0070: Cliclack UI Adoption

## Summary

Adopt the `cliclack` crate to provide a modern, consistent, and beautiful CLI user interface for `locald`. This replaces manual `println!` formatting and `indicatif` spinners with a unified design system inspired by `@clack/prompts`.

## Motivation

The current CLI output is functional but inconsistent. We use a mix of `println!`, `eprintln!`, and `indicatif` progress bars. The styling (emojis, colors) is manually managed in `style.rs` or scattered across handlers.

`cliclack` provides a "Task List" aesthetic (Intro -> Steps -> Outro) that is becoming standard in modern developer tools (Vercel, pnpm). It handles:

- Consistent spacing and indentation.
- Spinners for async tasks.
- Success/Failure states with appropriate symbols.
- Interactive prompts (if needed later).

## Design

### Dependencies

Add `cliclack` to `locald-cli`.

### Usage Patterns

1.  **Command Start**: Use `cliclack::intro` to announce the command context.
2.  **Long-running Tasks**: Use `cliclack::spinner` to show progress.
    ```rust
    let s = cliclack::spinner();
    s.start("Doing work...");
    // ...
    s.stop("Work done");
    ```
3.  **Instant Steps**: Use `cliclack::log::info`, `warn`, `error` for instantaneous feedback that fits the design system.
4.  **Command End**: Use `cliclack::outro` or `outro_note` to finish the session.

### Migration Strategy

1.  **Progress Renderer**: Refactor `locald-cli/src/progress.rs` to map `BootEvent`s to `cliclack` spinners.
2.  **Handlers**: Update `locald-cli/src/handlers.rs` to wrap commands in `intro`/`outro`.
3.  **Style**: Deprecate `locald-cli/src/style.rs` in favor of `cliclack`'s built-in styling where possible, or use `cliclack`'s primitives.

## Drawbacks

- `cliclack` is opinionated. If we want a very specific custom look, we might fight it. However, the "opinion" matches our desired aesthetic.
- Another dependency.

## Alternatives

- **Keep `indicatif`**: Powerful but requires heavy customization to match the "task list" look.
- **`dialoguer`**: Good for prompts, less for status reporting.

## Unresolved Questions

- How does `cliclack` handle parallel tasks? `locald` boot events can be parallel. `cliclack` is mostly sequential. We might need to serialize the display or use a multi-spinner approach if `cliclack` supports it (it might not support _concurrent_ spinners well, usually it's one active spinner).

  - _Resolution_: `locald` boot events are technically parallel but often displayed sequentially in a "step" list. If we have true parallelism, we might need to stick to `indicatif` `MultiProgress` or serialize the output.
  - _Refinement_: `locald`'s `BootEvent`s usually come in a stream. We can use `cliclack`'s `step` for completed items and a `spinner` for the _current_ active item. If multiple things happen at once, we might just log them.

  Actually, `locald` boot process is:

  1. Config (fast)
  2. Service A (start)
  3. Service B (start)

  These are often sequential in the `manager`. If they are parallel, `cliclack` might be tricky. `indicatif` handles `MultiProgress`.

  Let's check `locald-server`. `manager.rs` starts services sequentially in `apply_config` loop:

  ```rust
  for service_name in sorted_services {
     // ...
     // start service
     // wait for health
  }
  ```

  So it IS sequential! `cliclack` is perfect.
