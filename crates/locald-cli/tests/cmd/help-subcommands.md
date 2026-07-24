# locald init --help

```console
$ locald init --help
Initialize a new locald project

Usage: locald init [OPTIONS]

Options:
      --from-distribution <FROM_DISTRIBUTION>
          Initialize from a distribution archive (local path or URL)
      --sandbox <SANDBOX>
          Run in a sandbox environment
      --name <NAME>
          Project name (overrides prompt/default when using --from-distribution)
      --target <TARGET>
          Target directory (default: `./<project_name>`)
      --no-scaffold
          Skip scaffold files (only install plugins + locald.toml)
      --offline
          Use only bundled plugins, skip remote fetches
  -y, --yes
          Accept all defaults without prompting
  -v, --verbose
          Show detailed initialization steps
  -h, --help
          Print help

```

# locald up --help

```console
$ locald up --help
Ensure the current project is ready and print its URLs

Usage: locald up [OPTIONS] [PATH]

Arguments:
  [PATH]  Path to the service directory (defaults to current directory if locald.toml exists)

Options:
      --sandbox <SANDBOX>  Run in a sandbox environment
  -v, --verbose            Show verbose output
      --follow             Follow this project's logs after it becomes ready
  -h, --help               Print help

```

# locald down --help

```console
$ locald down --help
? 2
error: unrecognized subcommand 'down'

Usage: locald [OPTIONS] <COMMAND>

For more information, try '--help'.

```

# locald debug --help

```console
$ locald debug --help
Debugging tools

Usage: locald debug [OPTIONS] <COMMAND>

Commands:
  port      Check which process is listening on a port
  identity  Show CLI and daemon binary identity
  help      Print this message or the help of the given subcommand(s)

Options:
      --sandbox <SANDBOX>  Run in a sandbox environment
  -h, --help               Print help

```

# locald debug identity --help

```console
$ locald debug identity --help
Show CLI and daemon binary identity

Usage: locald debug identity [OPTIONS]

Options:
      --json               Machine-readable JSON output
      --sandbox <SANDBOX>  Run in a sandbox environment
  -h, --help               Print help

```
