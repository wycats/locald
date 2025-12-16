---
title: "Axiom: Respectful & Relevant Output"
---

**Output is a User Interface.** It is not a debug log. It must be relevant to the user's domain, respectful of their attention, and persistent enough to be useful after the fact.

## 1. The Filter: Relevance is Relative

Output is only "signal" if it matches the user's current **Persona** and **Intent**.

- **Define the Persona**: Avoid coarse-grained "verbosity" flags (`-v`, `-vv`) which are often arbitrary. Instead, offer flags that switch the persona:
  - **Default**: "App Builder" (What happened to my app?)
  - **`--debug-build`**: "Buildpack Author" (What did the buildpack script output?)
  - **`--trace-ipc`**: "Contributor" (What raw messages were sent?)
- **Respect the Domain**: If the user is an App Builder, internal buffer flushes are noise. If they are a Contributor, they are signal.

## 2. The Flow: Dynamic vs. Static

The terminal buffer is a timeline. We must distinguish between _what is happening now_ (Dynamic) and _what happened_ (Static).

### The "Fold Away" Rule (Dynamic)

While an operation is running, use high-fidelity, transient UI (spinners, progress bars) to show activity. When the operation completes, this UI must **fold away**, leaving no trace in the history.

- **Show Activity**: The user needs to know the system hasn't hung.
- **Don't Pollute**: Do not leave a trail of 100 progress updates (e.g., "Downloading layer...") in the terminal history.

### The "Look Away" Rule (Static)

The static text left behind must tell the complete story. A user should be able to walk away, come back, and understand exactly what succeeded or failed without having seen the animation.

- **History is Truth**: The final output is the permanent record.
- **No "Blink and You Miss It"**: Critical errors or warnings must persist. They must never be "folded away" with the progress bar.

## 3. The Crash Protocol: When Things Go Wrong

An unhandled error that dumps raw system information (stack traces, raw JSON, internal struct debug output) to the user is **always a bug**.

### The "Black Box" Rule

When an unexpected failure occurs that cannot be mapped to a domain-specific error message:

1.  **Capture Everything**: Gather the full context—stack trace, environment variables, recent logs, and the raw error.
2.  **Persist to Disk**: Write this "Black Box" data to a crash log file (e.g., `.locald/crashes/crash-TIMESTAMP.log`).
3.  **Inform Respectfully**: Display a concise, human-readable message to the user pointing to the log file.
    > ✖ An unexpected error occurred.
    > Details written to: .locald/crashes/crash-20231027-1030.log

### The Fallback

If—and only if—writing to the crash log fails (e.g., disk full, permission denied), the system must fallback to dumping the raw information to `stderr`. This is the "Nuclear Option" and is the only acceptable time to break the "Respectful" rule, as losing the error data entirely is worse.
