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
