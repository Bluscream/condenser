//! Emulator runtime, Proton discovery and source configuration commands.

use super::{flag, has_flag, split_flags, CliResult};
use condenser_core::config::{EmulatorSource, UmuSource};
use condenser_core::{gbe, proton, runtime, Engine};
use std::path::PathBuf;

pub(crate) fn dispatch(args: &[String]) -> CliResult {
    let Some((sub, rest)) = args.split_first() else {
        return Err("runtime: expected status, check, install or releases".into());
    };
    match sub.as_str() {
        "status" => status(rest),
        "check" => check(rest),
        "install" => install(rest),
        "releases" => releases(rest),
        other => Err(format!("unknown runtime command {other:?}").into()),
    }
}

fn status(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let engine = Engine::load()?;
    let st = runtime::status(&engine.paths.gbe_dir());

    if has_flag(&flags, "json") {
        println!("{}", serde_json::to_string_pretty(&st)?);
        return Ok(());
    }
    match st.installed {
        None => println!("no emulator installed — run `condenser runtime install`"),
        Some(m) => {
            println!("tag        {}", m.tag);
            println!("published  {}", m.published);
            println!(
                "age        {}",
                m.age_days().map_or_else(|| "-".to_owned(), |d| format!("{d} day(s)"))
            );
            println!(
                "platforms  {}",
                match (m.has_windows(), m.has_linux()) {
                    (true, true) => "windows + linux",
                    (true, false) => "windows",
                    (false, true) => "linux",
                    (false, false) => "none",
                }
            );
            println!("files      {}", m.assets.join(", "));
            println!("source     {}", m.source);
        }
    }
    Ok(())
}

fn check(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let engine = Engine::load()?;
    let st = runtime::check_for_update(&engine.paths.gbe_dir(), &engine.settings.emulator)?;

    if has_flag(&flags, "json") {
        println!("{}", serde_json::to_string_pretty(&st)?);
        return Ok(());
    }
    let latest = st.latest_tag.unwrap_or_default();
    match st.installed {
        Some(m) if !st.update_available => println!("up to date ({})", m.tag),
        Some(m) => println!("update available: {} -> {latest}", m.tag),
        None => println!("nothing installed; latest upstream is {latest}"),
    }
    Ok(())
}

fn install(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let platform: runtime::Platform = flag(&flags, "platform").unwrap_or("both").parse()?;
    let tag = flag(&flags, "tag").map(str::to_owned);

    let engine = Engine::load()?;
    let dir = engine.paths.gbe_dir();
    // Deployed libraries are held read-only; unlock so they can be replaced.
    gbe::unlock_runtime(&dir)?;

    let mut last = String::new();
    let mut progress = |stage: &str, done: u64, total: Option<u64>| {
        if stage != last {
            println!("  {stage}");
            last.clear();
            last.push_str(stage);
        }
        if let Some(total) = total {
            if done == total {
                println!("    {total} bytes");
            }
        }
    };
    let manifest = runtime::install(
        &dir,
        &engine.settings.emulator,
        tag.as_deref(),
        platform,
        &mut progress,
    )?;
    println!(
        "installed {} ({}): {}",
        manifest.tag,
        manifest.published,
        manifest.assets.join(", ")
    );

    // Existing deployments now point at replaced libraries; mark them for redeploy.
    let mut engine = Engine::load()?;
    for game in &mut engine.library.games {
        game.emu_deployed = false;
    }
    engine.save()?;
    Ok(())
}

fn releases(args: &[String]) -> CliResult {
    let (flags, _) = split_flags(args);
    let limit = flag(&flags, "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(15);
    let engine = Engine::load()?;
    for (tag, date) in runtime::list_releases(&engine.settings.emulator, limit)? {
        println!("{tag:<28}  {date}");
    }
    Ok(())
}

pub(crate) fn proton_dispatch(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    match positional.first().map(String::as_str) {
        Some("list") | None => {
            let builds = proton::discover();
            if has_flag(&flags, "json") {
                println!("{}", serde_json::to_string_pretty(&builds)?);
                return Ok(());
            }
            if builds.is_empty() {
                println!("no Proton builds found; umu will download one on first launch");
            }
            for build in builds {
                println!("{:<32}  {}", build.name, build.path.display());
            }
            Ok(())
        }
        Some(other) => Err(format!("unknown proton command {other:?}").into()),
    }
}

pub(crate) fn sources_dispatch(args: &[String]) -> CliResult {
    let (flags, positional) = split_flags(args);
    match positional.first().map(String::as_str) {
        Some("show") | None => sources_show(&flags),
        Some("set") => sources_set(&flags),
        Some("reset") => {
            let mut engine = Engine::load()?;
            engine.settings.emulator = EmulatorSource::default();
            engine.settings.umu = UmuSource::default();
            engine.save()?;
            println!("sources reset to defaults");
            Ok(())
        }
        Some(other) => Err(format!("unknown sources command {other:?}").into()),
    }
}

fn sources_show(flags: &[(String, String)]) -> CliResult {
    let engine = Engine::load()?;
    let emulator = &engine.settings.emulator;
    let umu = &engine.settings.umu;

    if has_flag(flags, "json") {
        let value = serde_json::json!({ "emulator": emulator, "umu": umu });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("emulator repo    {}", emulator.repo);
    println!("api base         {}", emulator.api_base);
    println!("windows asset    {}", emulator.windows_asset);
    println!("linux asset      {}", emulator.linux_asset);
    println!(
        "umu-run          {}",
        umu.run_path
            .as_ref()
            .map_or_else(|| "(search PATH)".to_owned(), |p| p.display().to_string())
    );
    println!("umu project      {}", umu.project_url);
    Ok(())
}

fn sources_set(flags: &[(String, String)]) -> CliResult {
    let mut engine = Engine::load()?;
    let mut changed = false;

    for (key, field) in [
        ("repo", 0_u8),
        ("api-base", 1),
        ("win-asset", 2),
        ("linux-asset", 3),
    ] {
        if let Some(value) = flag(flags, key) {
            let value = value.to_owned();
            match field {
                0 => engine.settings.emulator.repo = value,
                1 => engine.settings.emulator.api_base = value,
                2 => engine.settings.emulator.windows_asset = value,
                _ => engine.settings.emulator.linux_asset = value,
            }
            changed = true;
        }
    }
    if let Some(value) = flag(flags, "umu-run") {
        // An empty value clears the override and returns to searching PATH.
        engine.settings.umu.run_path =
            (!value.is_empty()).then(|| PathBuf::from(value));
        changed = true;
    }

    if !changed {
        return Err("nothing to set; see `condenser --help`".into());
    }
    engine.save()?;
    println!("sources updated");
    sources_show(&[])
}
