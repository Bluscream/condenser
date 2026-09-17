//! Library and per-game CLI commands.

use super::{flag, has_flag, require_arg, resolve_game, split_flags, CliResult};
use condenser_core::model::SteamMode;
use condenser_core::{gbe, umu, Engine};
use std::path::PathBuf;

/// Whole hours and minutes — truncation is the intent for a playtime readout.
#[allow(clippy::integer_division, reason = "whole hours/minutes are the desired units")]
fn fmt_playtime(seconds: u64) -> String {
    if seconds == 0 {
        return "never played".to_owned();
    }
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub(crate) fn list(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let engine = Engine::load()?;
    let games = engine.library.sorted();

    if has_flag(&flags, "json") {
        println!("{}", serde_json::to_string_pretty(&games)?);
        return Ok(());
    }
    if games.is_empty() {
        println!("library is empty — add a game with `condenser add <exe>`");
        return Ok(());
    }
    println!("{:<8}  {:<34}  {:<10}   PLAYTIME", "ID", "TITLE", "MODE");
    for game in games {
        let id = game.id.to_string();
        let short = id.get(..8).unwrap_or(&id);
        let mode = match game.steam_mode {
            SteamMode::ApiOnly => "api",
            SteamMode::ColdClient => "cold",
            SteamMode::None => "none",
        };
        let deployed = if game.emu_deployed { "*" } else { " " };
        println!(
            "{short}  {:<34}  {mode:<10}{deployed} {}",
            game.title,
            fmt_playtime(game.play_seconds)
        );
    }
    println!("\n* = emulator currently deployed");
    Ok(())
}

pub(crate) fn show(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    let engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let game = engine.library.get(id)?;

    if has_flag(&flags, "json") {
        println!("{}", serde_json::to_string_pretty(game)?);
        return Ok(());
    }
    println!("title           {}", game.title);
    println!("id              {}", game.id);
    println!("executable      {}", game.executable.display());
    println!("app id          {}", game.app_id.map_or_else(|| "-".to_owned(), |a| a.to_string()));
    println!("proton          {}", game.proton);
    println!("resolved proton {}", condenser_core::proton::resolve(&game.proton));
    println!("prefix          {}", game.prefix.display());
    println!("steamworks      {:?}", game.steam_mode);
    println!("emu deployed    {}", game.emu_deployed);
    println!("launch options  {}", if game.launch_options.is_empty() { "-" } else { &game.launch_options });
    println!("playtime        {}", fmt_playtime(game.play_seconds));
    Ok(())
}

pub(crate) fn add(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    let exe = PathBuf::from(require_arg(&positional, 0, "<exe>")?);
    if !exe.exists() {
        return Err(format!("no such executable: {}", exe.display()).into());
    }
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    let title = flag(&flags, "title").map_or_else(
        || {
            exe.file_stem()
                .map_or_else(|| "Untitled".to_owned(), |s| s.to_string_lossy().into_owned())
        },
        str::to_owned,
    );

    let mut engine = Engine::load()?;
    let paths = engine.paths.clone();
    let id = engine.library.add(title.clone(), exe, &paths);
    if let Some(app_id) = flag(&flags, "appid").and_then(|v| v.parse::<u32>().ok()) {
        engine.library.get_mut(id)?.app_id = Some(app_id);
    }
    engine.save()?;
    println!("added {title} ({id})");
    Ok(())
}

pub(crate) fn remove(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let game = engine.library.get(id)?.clone();
    // Always hand the game's own files back before forgetting about it.
    if let Err(e) = gbe::revert(&game) {
        eprintln!("warning: could not restore original files: {e}");
    }
    engine.library.remove(id)?;
    engine.save()?;
    println!("removed {}", game.title);
    Ok(())
}

pub(crate) fn play(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    println!("launching {}", engine.library.get(id)?.title);
    engine.play(id)?;
    Ok(())
}

pub(crate) fn preview(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let game = engine.library.get(id)?;
    let cmd = umu::build_command(game, &engine.settings)?;
    println!("{cmd:?}");
    Ok(())
}

pub(crate) fn set_options(args: &[String]) -> CliResult {
    // Deliberately *not* flag-parsed: launch options legitimately contain `--`,
    // `%command%` and `-flags`, all of which a flag parser would swallow.
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(args, 0, "<game>")?)?;
    let value = args.get(1..).unwrap_or_default().join(" ");
    engine.library.get_mut(id)?.launch_options.clone_from(&value);
    engine.save()?;
    println!(
        "launch options {}",
        if value.is_empty() { "cleared".to_owned() } else { format!("set to: {value}") }
    );
    Ok(())
}

pub(crate) fn set_proton(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let name = require_arg(&positional, 1, "<proton>")?.to_owned();
    let resolved = condenser_core::proton::resolve(&name);
    engine.library.get_mut(id)?.proton.clone_from(&name);
    engine.save()?;
    println!("proton set to {name}");
    if resolved == name {
        println!("note: no local build matched; umu will try to download it");
    } else {
        println!("resolves to {resolved}");
    }
    Ok(())
}

pub(crate) fn set_mode(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let mode = match require_arg(&positional, 1, "<api|cold|none>")? {
        "api" | "api-only" => SteamMode::ApiOnly,
        "cold" | "cold-client" => SteamMode::ColdClient,
        "none" => SteamMode::None,
        other => return Err(format!("unknown mode {other:?} (api, cold or none)").into()),
    };
    engine.library.get_mut(id)?.steam_mode = mode;
    engine.save()?;
    println!("steamworks mode set to {mode:?}");
    Ok(())
}

pub(crate) fn deploy(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let gbe_dir = engine.paths.gbe_dir();
    let game = engine.library.get(id)?.clone();
    let config = engine.settings.emu_config.merged_with(&game.emu_config);
    gbe::deploy(&game, &gbe_dir, &config)?;
    engine.library.get_mut(id)?.emu_deployed = true;
    engine.save()?;
    println!("deployed emulator into {}", game.game_dir().display());
    Ok(())
}

pub(crate) fn revert(args: &[String]) -> CliResult {
    let (_, positional) = split_flags(args);
    let mut engine = Engine::load()?;
    let id = resolve_game(&engine, require_arg(&positional, 0, "<game>")?)?;
    let game = engine.library.get(id)?.clone();
    gbe::revert(&game)?;
    engine.library.get_mut(id)?.emu_deployed = false;
    engine.save()?;
    println!("restored original files in {}", game.game_dir().display());
    Ok(())
}

pub(crate) fn revert_all() -> CliResult {
    let mut engine = Engine::load()?;
    let games = engine.library.games.clone();
    for game in games {
        match gbe::revert(&game) {
            Ok(()) => println!("restored {}", game.title),
            Err(e) => eprintln!("{}: {e}", game.title),
        }
        engine.library.get_mut(game.id)?.emu_deployed = false;
    }
    engine.save()?;
    Ok(())
}
