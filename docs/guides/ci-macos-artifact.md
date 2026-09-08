# Verified macOS CI artifact

The existing **macOS Build** CI job builds an Apple Silicon stable binary when Rust or embedded dashboard/docs inputs change. It runs the release binary's version/help smoke checks, preserves those exact bytes, then runs both stable and all-feature unit suites. Only successful completion packages and uploads `locald-macos-aarch64-<run-id>-<attempt>` for 14 days. The packaging step rejects replacement of the smoke-tested stable binary, even if later test steps build another channel.

The artifact contains:

- `locald-macos-aarch64.tar.gz`: executable `locald` and `manifest.json`. Tar preserves executable permissions; packaging verifies the archived bytes and mode.
- `artifact.json`: binary and archive SHA-256 checksums, actual checked-out Git commit/tree, Rust compiler/host, verified Mach-O architecture, channel and GitHub run provenance.
- `SHA256SUMS`: checksums for the archive and outer metadata.

The CLI embeds its macOS agent/helper payloads and built dashboard/docs assets, so the runtime payload is one binary. This CI artifact is not a public release or notarization claim. Checksums establish artifact identity, not an independent signature.

For a pull request, `checkout.commit` normally names GitHub's synthetic merge commit. It is deliberately obtained from Git rather than labeled as the PR head. Use a successful main-branch run for rollout of a merged change, and verify the desired commit/tree, run provenance, `aarch64-apple-darwin` host, `arm64` architecture and checksums before consuming its artifact. Extract the tar rather than treating the upload wrapper as the executable; confirm `locald` still has execute permission and matches `binary.sha256`.

Artifact production does not install or activate locald. Download, installed-path replacement, helper/admin setup and daemon restart remain separate rollout decisions. Local packaging fixtures do not execute locald; real binary compilation and smoke/unit validation happen in GitHub CI.
