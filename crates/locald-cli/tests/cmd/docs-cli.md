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

# locald status --help

```console
$ locald status --help
Explain desired and actual project availability

Usage: locald status [OPTIONS] [PATH]

Arguments:
  [PATH]  Project path (defaults to the current directory)

Options:
      --all                Show all known projects instead of the current project
      --sandbox <SANDBOX>  Run in a sandbox environment
      --json               Machine-readable JSON output
  -h, --help               Print help

```

# locald try --help

```console
$ locald try --help
Experiment with a command (attached). On exit, prompts to save to locald.toml.

This command runs the specified command in the current terminal. It injects a dynamic PORT and sets up the environment. When the command exits (e.g. via Ctrl-C), you will be asked if you want to save it as a permanent service in your locald.toml.

Usage: locald try [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...
          Command to run

Options:
      --sandbox <SANDBOX>
          Run in a sandbox environment

  -h, --help
          Print help (see a summary with '-h')

```
