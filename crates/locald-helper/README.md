# locald-helper

macOS privileged helper daemon for locald. Runs as a LaunchDaemon and performs
two narrowly scoped operations that require root: binding locald's listeners on
ports 80/443 and atomically replacing locald's complete managed `/etc/hosts`
section. CA trust changes remain part of the explicit `sudo locald admin setup`
flow and are not exposed by the long-running helper.

The helper communicates with the installed `locald` binary through authenticated,
versioned XPC on the `com.locald.helper` Mach service.
