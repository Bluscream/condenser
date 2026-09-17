//! Headless control of Condenser.
//!
//! Lives in the same binary as the GUI: running `condenser` with no arguments opens the
//! window, and any argument switches to command mode. Everything the UI can do is
//! reachable here, so the launcher is scriptable without a display.

// `unreachable_pub` wants pub(crate) here while the nursery's `redundant_pub_crate`
// wants plain pub; the former is the more useful of the two, so the latter is muted
// for this module tree.
#![allow(clippy::redundant_pub_crate)]

use condenser_core::{Engine, Game};
use uuid::Uuid;

pub(crate) mod config_cmds;
pub(crate) mod games;
pub(crate) mod runtime_cmds;

pub(crate) type CliResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) const HELP: &str = "\
condenser — run Steam games through Proton without the Steam client

USAGE
  condenser                              open the graphical launcher
  condenser <command> [args]             run headless

GAMES
  list [--json]                          list the library
  show <game>                            everything known about one game
  add <exe> [--title T] [--appid N]      add a game
  remove <game>                          remove (restores original files first)
  play <game>                            deploy if needed, then launch
  preview <game>                         print the launch command, run nothing

SETTINGS
  set-options <game> <string>            Steam-style launch options ('' to clear)
  set-proton <game> <name>               Proton build for this game
  set-mode <game> <api|cold|none>        Steamworks emulation mode

EMULATOR
  deploy <game>                          inject the emulator
  revert <game>                          restore the game's original files
  revert-all                             restore every game in the library

EMULATOR CONFIG                          (omit --game to act on the global layer)
  config show [--game G] [--json]        effective settings and where each came from
  config get <key> [--game G]            one value
  config set <key> <value> [--game G]    set it
  config unset <key> [--game G]          remove it
  config keys [--json]                   commonly used keys

RUNTIME
  runtime status [--json]                installed emulator release
  runtime check                          compare against upstream
  runtime install [--platform P] [--tag T]   P = windows | linux | both
  runtime releases [--limit N]           recent upstream releases

SYSTEM
  proton list [--json]                   Proton builds found on this machine
  sources show [--json]                  where umu and the emulator come from
  sources set [--repo R] [--api-base U] [--win-asset S] [--linux-asset S] [--umu-run P]
  sources reset                          back to upstream defaults

<game> matches an id prefix, or a unique case-insensitive part of the title.
";

/// Run a CLI invocation. `args` excludes the program name.
pub(crate) fn run(args: &[String]) -> CliResult {
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return Ok(());
    };

    match command.as_str() {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("condenser {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "list" => games::list(rest),
        "show" => games::show(rest),
        "add" => games::add(rest),
        "remove" => games::remove(rest),
        "play" => games::play(rest),
        "preview" => games::preview(rest),
        "set-options" => games::set_options(rest),
        "set-proton" => games::set_proton(rest),
        "set-mode" => games::set_mode(rest),
        "deploy" => games::deploy(rest),
        "revert" => games::revert(rest),
        "revert-all" => games::revert_all(),
        "config" => config_cmds::dispatch(rest),
        "runtime" => runtime_cmds::dispatch(rest),
        "proton" => runtime_cmds::proton_dispatch(rest),
        "sources" => runtime_cmds::sources_dispatch(rest),
        other => Err(format!("unknown command {other:?}; try `condenser --help`").into()),
    }
}

// --- shared helpers -------------------------------------------------------

/// Read `--flag value` pairs, returning them plus the untouched positional arguments.
pub(crate) fn split_flags(args: &[String]) -> (Vec<(String, String)>, Vec<String>) {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // A bare `--` ends flag parsing, so values containing dashes survive intact.
        if arg == "--" {
            positional.extend(iter.cloned());
            break;
        }
        if let Some(name) = arg.strip_prefix("--") {
            // Boolean flags carry an empty value.
            match iter.clone().next() {
                Some(next) if !next.starts_with("--") => {
                    flags.push((name.to_owned(), next.clone()));
                    iter.next();
                }
                _ => flags.push((name.to_owned(), String::new())),
            }
        } else {
            positional.push(arg.clone());
        }
    }
    (flags, positional)
}

pub(crate) fn flag<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

pub(crate) fn has_flag(flags: &[(String, String)], name: &str) -> bool {
    flags.iter().any(|(k, _)| k == name)
}

/// Find one game by id prefix or a unique part of its title.
pub(crate) fn resolve_game(engine: &Engine, needle: &str) -> Result<Uuid, Box<dyn std::error::Error>> {
    let lower = needle.to_lowercase();
    let matches: Vec<&Game> = engine
        .library
        .games
        .iter()
        .filter(|g| {
            g.id.to_string().starts_with(needle) || g.title.to_lowercase().contains(&lower)
        })
        .collect();

    match matches.as_slice() {
        [one] => Ok(one.id),
        [] => Err(format!("no game matches {needle:?}").into()),
        many => {
            let titles: Vec<&str> = many.iter().map(|g| g.title.as_str()).collect();
            Err(format!("{needle:?} matches {} games: {}", many.len(), titles.join(", ")).into())
        }
    }
}

pub(crate) fn require_arg<'a>(positional: &'a [String], index: usize, what: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    positional
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing argument: {what}").into())
}
