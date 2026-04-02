# locald-helper

macOS privileged helper daemon for locald. Runs as a LaunchDaemon and performs
operations that require root: pfctl port forwarding and CA trust installation.

Communicates with the locald agent via XPC (Mach service `com.locald.helper`).
