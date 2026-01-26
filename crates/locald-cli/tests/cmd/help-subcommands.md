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
Start the daemon (if needed) and register the current project

Usage: locald up [OPTIONS] [PATH]

Arguments:
  [PATH]  Path to the service directory (defaults to current directory if locald.toml exists)

Options:
      --sandbox <SANDBOX>  Run in a sandbox environment
  -v, --verbose            Show verbose output
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
