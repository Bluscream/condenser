use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Resolved on-disk locations for Condenser data.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Root data dir, e.g. ~/.local/share/condenser
    pub data: PathBuf,
    /// Root config dir, e.g. ~/.config/condenser
    pub config: PathBuf,
}

impl AppPaths {
    #[must_use]
    pub fn discover() -> Self {
        if let Some(pd) = ProjectDirs::from("dev", "condenser", "Condenser") {
            Self {
                data: pd.data_dir().to_path_buf(),
                config: pd.config_dir().to_path_buf(),
            }
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            Self {
                data: Path::new(&home).join(".local/share/condenser"),
                config: Path::new(&home).join(".config/condenser"),
            }
        }
    }

    #[must_use]
    pub fn library_file(&self) -> PathBuf {
        self.data.join("library.json")
    }
    #[must_use]
    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }
    /// Per-game Proton prefixes live here.
    #[must_use]
    pub fn prefixes_dir(&self) -> PathBuf {
        self.data.join("prefixes")
    }
    /// Downloaded/vendored `gbe_fork` runtime.
    #[must_use]
    pub fn gbe_dir(&self) -> PathBuf {
        self.data.join("gbe_fork")
    }
    /// Cached cover/hero/logo art.
    #[must_use]
    pub fn artwork_dir(&self) -> PathBuf {
        self.data.join("artwork")
    }

    /// Create every directory Condenser writes into.
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [
            &self.data,
            &self.config,
            &self.prefixes_dir(),
            &self.gbe_dir(),
            &self.artwork_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// Where the Steamworks emulator is fetched from.
///
/// Configurable so a different `gbe_fork` fork — or a private mirror — can be used
/// without patching the binary. Assets are matched by substring, since forks tend to
/// keep upstream naming while adding their own suffixes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmulatorSource {
    /// GitHub `owner/repo` to pull releases from.
    pub repo: String,
    /// Substring identifying the Windows archive asset.
    pub windows_asset: String,
    /// Substring identifying the Linux archive asset.
    pub linux_asset: String,
    /// API base URL, so a GitHub Enterprise host or mirror can be targeted.
    pub api_base: String,
}

impl Default for EmulatorSource {
    fn default() -> Self {
        Self {
            repo: "Detanup01/gbe_fork".into(),
            windows_asset: "emu-win-release".into(),
            linux_asset: "emu-linux-release".into(),
            api_base: "https://api.github.com".into(),
        }
    }
}

/// Where `umu-run` comes from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UmuSource {
    /// Explicit path to `umu-run`; `None` searches `PATH` and the usual install spots.
    pub run_path: Option<PathBuf>,
    /// Project the user is pointed at when `umu-run` cannot be found.
    pub project_url: String,
}

impl Default for UmuSource {
    fn default() -> Self {
        Self {
            run_path: None,
            project_url: "https://github.com/Open-Wine-Components/umu-launcher".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Where the Proton runtime launcher comes from.
    #[serde(default)]
    pub umu: UmuSource,
    /// Where the Steamworks emulator comes from.
    #[serde(default)]
    pub emulator: EmulatorSource,
    /// Emulator settings applied to every game; per-game entries override these.
    #[serde(default = "crate::emu_config::defaults")]
    pub emu_config: crate::EmuConfig,
    /// Default Proton build for new games.
    pub default_proton: String,
    /// `SteamGridDB` API key for artwork.
    pub steamgriddb_key: Option<String>,
    /// Anonymous Steam appinfo endpoint used by config generation.
    pub emu_generate_online: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            umu: UmuSource::default(),
            emulator: EmulatorSource::default(),
            emu_config: crate::emu_config::defaults(),
            default_proton: "GE-Proton".into(),
            steamgriddb_key: None,
            emu_generate_online: true,
        }
    }
}
