//! Emulator configuration commands: global and per-game.

use super::{flag, has_flag, resolve_game, split_flags, CliResult};
use condenser_core::emu_config;
use condenser_core::Engine;

pub(crate) fn dispatch(args: &[String]) -> CliResult {
    let Some((sub, rest)) = args.split_first() else {
        return Err("config: expected show, get, set, unset or keys".into());
    };
    match sub.as_str() {
        "show" => show(rest),
        "get" => get(rest),
        "set" => set(rest),
        "unset" => unset(rest),
        "keys" => keys(rest),
        other => Err(format!("unknown config command {other:?}").into()),
    }
}

/// Resolve which layer a command targets: `--game X` selects a game, otherwise global.
fn layer(engine: &Engine, flags: &[(String, String)]) -> Result<Option<uuid::Uuid>, Box<dyn std::error::Error>> {
    match flag(flags, "game") {
        Some(needle) if !needle.is_empty() => Ok(Some(resolve_game(engine, needle)?)),
        _ => Ok(None),
    }
}

fn show(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let engine = Engine::load()?;
    let target = layer(&engine, &flags)?;

    let global = &engine.settings.emu_config;
    let (per_game, effective) = match target {
        Some(id) => {
            let game = engine.library.get(id)?;
            (Some(&game.emu_config), global.merged_with(&game.emu_config))
        }
        None => (None, global.clone()),
    };

    if has_flag(&flags, "json") {
        let value = serde_json::json!({
            "global": global,
            "game": per_game,
            "effective": effective,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if let Some(id) = target {
        println!("game: {}\n", engine.library.get(id)?.title);
    }
    if effective.is_empty() {
        println!("no emulator settings configured");
        return Ok(());
    }
    println!("{:<48}  {:<22}  SOURCE", "KEY", "VALUE");
    for (key, value) in &effective.entries {
        let source = match per_game {
            Some(pg) if pg.get(key).is_some() => "game",
            _ => "global",
        };
        println!("{key:<48}  {value:<22}  {source}");
    }
    if target.is_some() {
        println!("\nper-game entries override global ones");
    }
    Ok(())
}

fn get(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    let key = positional.first().ok_or("missing argument: <key>")?;
    let engine = Engine::load()?;
    let effective = match layer(&engine, &flags)? {
        Some(id) => engine
            .settings
            .emu_config
            .merged_with(&engine.library.get(id)?.emu_config),
        None => engine.settings.emu_config.clone(),
    };
    match effective.get(key) {
        Some(value) => println!("{value}"),
        None => return Err(format!("{key} is not set").into()),
    }
    Ok(())
}

fn set(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    let key = positional.first().ok_or("missing argument: <key>")?.clone();
    // Join the remainder so values with spaces need no quoting.
    let value = positional.get(1..).unwrap_or_default().join(" ");
    if value.is_empty() {
        return Err("missing argument: <value> (use `config unset` to remove)".into());
    }

    let mut engine = Engine::load()?;
    if let Some(id) = layer(&engine, &flags)? {
        engine.library.get_mut(id)?.emu_config.set(&key, &value)?;
        println!("set {key}={value} for {}", engine.library.get(id)?.title);
    } else {
        engine.settings.emu_config.set(&key, &value)?;
        println!("set {key}={value} globally");
    }
    engine.save()?;
    println!("takes effect on the next deploy — run `condenser deploy <game>` to apply now");
    Ok(())
}

fn unset(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    let key = positional.first().ok_or("missing argument: <key>")?.clone();

    let mut engine = Engine::load()?;
    let removed = match layer(&engine, &flags)? {
        Some(id) => engine.library.get_mut(id)?.emu_config.unset(&key),
        None => engine.settings.emu_config.unset(&key),
    };
    engine.save()?;
    if removed {
        println!("removed {key}");
    } else {
        println!("{key} was not set at this level");
    }
    Ok(())
}

fn keys(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    if has_flag(&flags, "json") {
        let value: Vec<_> = emu_config::COMMON_KEYS
            .iter()
            .map(|(k, d)| serde_json::json!({ "key": k, "description": d }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("Commonly used keys (any `<user|main|app|overlay>::<section>::<key>` works):\n");
    for (key, description) in emu_config::COMMON_KEYS {
        println!("{key:<52}  {description}");
    }
    println!(
        "\nFull reference: https://github.com/Detanup01/gbe_fork \
         (post_build/steam_settings.EXAMPLE)"
    );
    Ok(())
}
