# locald --help

```console
$ locald --help
Local development proxy and process manager

Usage: locald [OPTIONS] <COMMAND>

Commands:
  init         Initialize a new locald project
  try          Experiment with a command (attached). On exit, prompts to save to locald.toml
  run          Run a one-off task in the context of a service (with injected environment)
  add          Add a service to locald.toml (shortcut for `locald service add exec`)
  service      Manage services
  monitor      Monitor running services (TUI)
  ping         Ping the locald daemon
  trust        Install the locald Root CA into the system trust store
  server       Server management commands
  selfupgrade  Self-upgrade locald to a newer version
  up           Start the daemon (if needed) and register the current project
  dashboard    Open the dashboard in the default browser
  doctor       Diagnose host readiness for running locald
  stop         Stop a running service. If no name is provided, stops all services defined in locald.toml in the current directory
  restart      Restart a running service
  status       List running services
  logs         Stream logs from services
  admin        Administrative commands
  ai           AI integration commands
  debug        Debugging tools
  config       Configuration management
  registry     Registry management commands
  serve        Serve a directory via HTTP
  help         Print this message or the help of the given subcommand(s)

Options:
      --sandbox <SANDBOX>  Run in a sandbox environment
  -h, --help               Print help
  -V, --version            Print version

```
