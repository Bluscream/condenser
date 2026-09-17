//! Condenser engine: run Steam games through Proton without the Steam client.
//!
//! Composes two upstream projects:
//!   * **umu-launcher** — Proton + Steam Linux Runtime container (the runtime).
//!   * **`gbe_fork`** — Steamworks API emulator (achievements, DLC, LAN, `SteamID`).
//!
//! This crate is deliberately GUI-free so it builds and tests on an immutable host
//! without webkit2gtk; the Tauri shell depends on it.

pub mod config;
pub mod error;
pub mod gbe;
pub mod launch_options;
pub mod library;
pub mod model;
pub mod proton;
pub mod runtime;
pub mod umu;

pub use config::{AppPaths, Settings};
pub use error::{Error, Result};
pub use library::Library;
pub use model::{Artwork, Game, SteamMode};

/// High-level facade the UI layer talks to.
#[derive(Debug)]
pub struct Engine {
    pub paths: AppPaths,
    pub settings: Settings,
    pub library: Library,
}

impl Engine {
    /// Load paths, settings and library from disk, creating defaults as needed.
    pub fn load() -> Result<Self> {
        let paths = AppPaths::discover();
        paths.ensure()?;
        let settings = match std::fs::read_to_string(paths.settings_file()) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Settings::default(),
        };
        let library = Library::load(&paths)?;
        Ok(Self {
            paths,
            settings,
            library,
        })
    }

    pub fn save(&self) -> Result<()> {
        self.library.save(&self.paths)?;
        std::fs::write(
            self.paths.settings_file(),
            serde_json::to_string_pretty(&self.settings)?,
        )?;
        Ok(())
    }

    /// Prepare a game (deploy the emulator) and launch it, recording playtime.
    pub fn play(&mut self, id: uuid::Uuid) -> Result<()> {
        let gbe_dir = self.paths.gbe_dir();
        let game = self.library.get(id)?.clone();

        if game.steam_mode != SteamMode::None && !game.emu_deployed {
            gbe::deploy(&game, &gbe_dir)?;
            self.library.get_mut(id)?.emu_deployed = true;
        }

        let elapsed = umu::launch_blocking(&game, &self.settings)?;

        let g = self.library.get_mut(id)?;
        g.play_seconds += elapsed.as_secs();
        g.last_played = Some(now_unix());
        self.save()
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
