// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// `#[tauri::command]` dictates these shapes: handlers must take owned arguments, and
// each one holds its MutexGuard for the length of the call by design. Scoped to this
// crate so the engine keeps the stricter workspace defaults.
#![allow(clippy::needless_pass_by_value, clippy::significant_drop_tightening)]

mod cli;

use condenser_core::{gbe, model::SteamMode, runtime, Engine, Game};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

struct AppState(Mutex<Engine>);

/// Shape the frontend renders. Flattening keeps the JS side simple and avoids
/// leaking engine internals into the UI.
#[derive(Serialize)]
struct GameView {
    id: String,
    title: String,
    app_id: Option<u32>,
    executable: String,
    proton: String,
    steam_mode: SteamMode,
    emu_deployed: bool,
    play_seconds: u64,
    last_played: Option<i64>,
    launch_options: String,
    cover: Option<String>,
}

impl From<&Game> for GameView {
    fn from(g: &Game) -> Self {
        Self {
            id: g.id.to_string(),
            title: g.title.clone(),
            app_id: g.app_id,
            executable: g.executable.display().to_string(),
            proton: g.proton.clone(),
            steam_mode: g.steam_mode,
            emu_deployed: g.emu_deployed,
            play_seconds: g.play_seconds,
            last_played: g.last_played,
            launch_options: g.launch_options.clone(),
            cover: g.artwork.cover.as_ref().map(|p| p.display().to_string()),
        }
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn list_games(state: State<'_, AppState>) -> Result<Vec<GameView>, String> {
    let engine = state.0.lock().map_err(err)?;
    Ok(engine.library.sorted().into_iter().map(GameView::from).collect())
}

#[tauri::command]
fn add_game(
    state: State<'_, AppState>,
    title: String,
    executable: String,
    app_id: Option<u32>,
) -> Result<GameView, String> {
    let mut engine = state.0.lock().map_err(err)?;
    let paths = engine.paths.clone();
    let id = engine.library.add(title, PathBuf::from(executable), &paths);
    if let Some(app_id) = app_id {
        engine.library.get_mut(id).map_err(err)?.app_id = Some(app_id);
    }
    engine.save().map_err(err)?;
    let game = engine.library.get(id).map_err(err)?;
    Ok(GameView::from(game))
}

#[tauri::command]
fn remove_game(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    // Always restore the game's original DLLs before forgetting about it.
    if let Ok(game) = engine.library.get(uuid) {
        let _ = gbe::revert(&game.clone());
    }
    engine.library.remove(uuid).map_err(err)?;
    engine.save().map_err(err)
}

/// Deploy `gbe_fork` without launching — lets the user verify injection first.
#[tauri::command]
fn deploy_emu(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    let gbe_dir = engine.paths.gbe_dir();
    let game = engine.library.get(uuid).map_err(err)?.clone();
    gbe::deploy(&game, &gbe_dir).map_err(err)?;
    engine.library.get_mut(uuid).map_err(err)?.emu_deployed = true;
    engine.save().map_err(err)
}

#[tauri::command]
fn revert_emu(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    let game = engine.library.get(uuid).map_err(err)?.clone();
    gbe::revert(&game).map_err(err)?;
    engine.library.get_mut(uuid).map_err(err)?.emu_deployed = false;
    engine.save().map_err(err)
}

/// Launch a game. Runs on a blocking thread so the UI stays responsive while the
/// game is alive (umu blocks for the lifetime of the process).
#[tauri::command]
async fn play_game(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut engine = state.0.lock().map_err(err)?;
        engine.play(uuid).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn set_steam_mode(state: State<'_, AppState>, id: String, mode: SteamMode) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    engine.library.get_mut(uuid).map_err(err)?.steam_mode = mode;
    engine.save().map_err(err)
}

// --- `gbe_fork` runtime management ----------------------------------------

/// Local-only status: which `gbe_fork` release is installed, if any.
#[tauri::command]
fn runtime_status(state: State<'_, AppState>) -> Result<runtime::RuntimeStatus, String> {
    let engine = state.0.lock().map_err(err)?;
    Ok(runtime::status(&engine.paths.gbe_dir()))
}

/// Online freshness check against upstream releases.
#[tauri::command]
async fn runtime_check_update(app: tauri::AppHandle) -> Result<runtime::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (gbe_dir, emulator) = {
            let state = app.state::<AppState>();
            let engine = state.0.lock().map_err(err)?;
            (engine.paths.gbe_dir(), engine.settings.emulator.clone())
        };
        runtime::check_for_update(&gbe_dir, &emulator).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Fetch and install `gbe_fork`. Blocking: downloads and extracts an archive.
#[tauri::command]
async fn runtime_install(
    app: tauri::AppHandle,
    tag: Option<String>,
    platform: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let platform: runtime::Platform = platform
            .unwrap_or_else(|| "windows".into())
            .parse()
            .map_err(err)?;
        let (gbe_dir, emulator) = {
            let state = app.state::<AppState>();
            let engine = state.0.lock().map_err(err)?;
            (engine.paths.gbe_dir(), engine.settings.emulator.clone())
        };
        // Deployed libraries are locked read-only; unlock so they can be replaced.
        gbe::unlock_runtime(&gbe_dir).map_err(err)?;

        let mut progress = |stage: &str, done: u64, total: Option<u64>| {
            let _ = app.emit(
                "runtime-progress",
                serde_json::json!({ "stage": stage, "done": done, "total": total }),
            );
        };
        let manifest =
            runtime::install(&gbe_dir, &emulator, tag.as_deref(), platform, &mut progress).map_err(err)?;
        let out = format!("installed {} ({})", manifest.tag, manifest.published);
        // Newly installed libraries invalidate every prior deployment, so the next
        // launch of each game re-deploys from the fresh archive.
        let state = app.state::<AppState>();
        let mut engine = state.0.lock().map_err(err)?;
        for game in &mut engine.library.games {
            game.emu_deployed = false;
        }
        engine.save().map_err(err)?;
        Ok(out)
    })
    .await
    .map_err(err)?
}

/// Steam-style launch options, e.g. `gamescope -f -- %command%`.
#[tauri::command]
fn set_launch_options(state: State<'_, AppState>, id: String, options: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    engine.library.get_mut(uuid).map_err(err)?.launch_options = options;
    engine.save().map_err(err)
}

/// Proton builds found on this machine, for the per-game picker.
#[tauri::command]
fn list_protons() -> Vec<condenser_core::proton::ProtonBuild> {
    condenser_core::proton::discover()
}

#[tauri::command]
fn set_proton(state: State<'_, AppState>, id: String, proton: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(err)?;
    let mut engine = state.0.lock().map_err(err)?;
    engine.library.get_mut(uuid).map_err(err)?.proton = proton;
    engine.save().map_err(err)
}

/// Current source configuration for umu and the emulator.
#[tauri::command]
fn get_sources(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let engine = state.0.lock().map_err(err)?;
    Ok(serde_json::json!({
        "emulator": engine.settings.emulator,
        "umu": engine.settings.umu,
    }))
}

/// Point Condenser at a different emulator fork or umu build.
#[tauri::command]
fn set_sources(
    state: State<'_, AppState>,
    emulator: Option<condenser_core::config::EmulatorSource>,
    umu: Option<condenser_core::config::UmuSource>,
) -> Result<(), String> {
    let mut engine = state.0.lock().map_err(err)?;
    if let Some(emulator) = emulator {
        engine.settings.emulator = emulator;
    }
    if let Some(umu) = umu {
        engine.settings.umu = umu;
    }
    engine.save().map_err(err)
}

fn main() -> std::process::ExitCode {
    // Any argument switches to headless mode; bare invocation opens the window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return match cli::run(&args) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("condenser: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("condenser: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::load()?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState(Mutex::new(engine)))
        .invoke_handler(tauri::generate_handler![
            list_games,
            add_game,
            remove_game,
            deploy_emu,
            revert_emu,
            play_game,
            set_steam_mode,
            runtime_status,
            runtime_check_update,
            runtime_install,
            set_launch_options,
            list_protons,
            set_proton,
            get_sources,
            set_sources,
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
