# locald --help

```console
$ locald --help
Local development proxy and process manager

Usage: locald [OPTIONS] <COMMAND>

Commands:
  init         Initialize a new locald project
  try          Experiment with a command (attached). On exit, prompts to save to locald.toml
  run          Run a one-off task in the context of a service (with injected environment)
  add          Add a service to locald.toml
  service      Manage services
  monitor      Monitor running services (TUI)
  ping         Ping the locald daemon
  trust        Install the locald Root CA into the system trust store
  server       Server management commands
  selfupgrade  Self-upgrade locald to a newer version
  up           Ensure the current project is ready and print its URLs
  dashboard    Open the dashboard in the default browser
  doctor       Diagnose host readiness for running locald
  stop         Pause the current project. With a name, stop only that service
  pin          Keep a project available even without an active demand
  unpin        Return a project to automatic demand-based availability
  restart      Restart a running service
  status       Explain desired and actual project availability
  logs         Stream logs from services
  admin        Administrative commands
  tray         Manage the menu bar agent
  ai           AI integration commands
  debug        Debugging tools
  config       Configuration management
  registry     Registry management commands
  project      Project lifecycle management (plumbing)
  serve        Serve a directory via HTTP
  help         Print this message or the help of the given subcommand(s)

Options:
      --sandbox <SANDBOX>  Run in a sandbox environment
  -h, --help               Print help
  -V, --version            Print version

```
