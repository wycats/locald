use clap::{Arg, Command};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CliManifest {
    pub schema_version: u32,
    pub root: CommandManifest,
}

#[derive(Debug, Serialize)]
pub struct CommandManifest {
    pub name: String,
    pub aliases: Vec<String>,
    pub hidden: bool,
    pub args: Vec<ArgManifest>,
    pub subcommands: Vec<Self>,
}

#[derive(Debug, Serialize)]
pub struct ArgManifest {
    pub long: Option<String>,
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub global: bool,
    pub hidden: bool,
    pub positional: bool,
}

pub fn from_clap_command(mut root: Command) -> CliManifest {
    // Ensure determinism: disable help subcommand auto-generation differences.
    // (clap still may add help/version args; we record what clap reports.)
    root.build();

    CliManifest {
        schema_version: 1,
        root: command_manifest(&root),
    }
}

fn command_manifest(cmd: &Command) -> CommandManifest {
    let mut aliases: Vec<String> = cmd
        .get_all_aliases()
        .map(std::string::ToString::to_string)
        .collect();
    aliases.sort();
    aliases.dedup();

    let mut args: Vec<ArgManifest> = cmd.get_arguments().map(arg_manifest).collect();
    args.sort_by(|a, b| (a.positional, &a.long, a.short).cmp(&(b.positional, &b.long, b.short)));

    let mut subcommands: Vec<CommandManifest> =
        cmd.get_subcommands().map(command_manifest).collect();
    subcommands.sort_by(|a, b| a.name.cmp(&b.name));

    CommandManifest {
        name: cmd.get_name().to_string(),
        aliases,
        hidden: cmd.is_hide_set(),
        args,
        subcommands,
    }
}

fn arg_manifest(arg: &Arg) -> ArgManifest {
    let mut aliases: Vec<String> = arg
        .get_visible_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(std::string::ToString::to_string)
        .collect();
    aliases.sort();
    aliases.dedup();

    ArgManifest {
        long: arg.get_long().map(std::string::ToString::to_string),
        short: arg.get_short(),
        aliases,
        global: arg.is_global_set(),
        hidden: arg.is_hide_set(),
        positional: arg.get_index().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_manifest_is_deterministic_for_same_cli() {
        let manifest1 = from_clap_command(Cli::command());
        let manifest2 = from_clap_command(Cli::command());

        let json1 = serde_json::to_string(&manifest1).unwrap();
        let json2 = serde_json::to_string(&manifest2).unwrap();

        assert_eq!(json1, json2);
    }

    #[test]
    fn cli_manifest_includes_hidden_surface_group() {
        let manifest = from_clap_command(Cli::command());

        let surface = manifest
            .root
            .subcommands
            .iter()
            .find(|cmd| cmd.name == "__surface")
            .expect("expected __surface internal command");

        assert!(surface.hidden);

        let cli_manifest_cmd = surface
            .subcommands
            .iter()
            .find(|cmd| cmd.name == "cli-manifest")
            .expect("expected __surface cli-manifest subcommand");

        assert!(!cli_manifest_cmd.hidden);
    }
}
