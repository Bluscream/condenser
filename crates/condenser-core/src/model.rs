use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// How a game presents its Steamworks integration, which decides how we inject `gbe_fork`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SteamMode {
    /// Game links `steam_api(64).dll` only — a plain DLL swap is enough. Most common.
    #[default]
    ApiOnly,
    /// Game expects a running Steam client (steamclient) — use `gbe_fork`'s `ColdClientLoader`.
    ColdClient,
    /// DRM-free / no Steamworks — launch straight through umu with no emu.
    None,
}

/// A single library entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: Uuid,
    /// Steam `AppID`, if known. Drives emu config generation and artwork lookup.
    pub app_id: Option<u32>,
    pub title: String,
    /// The Windows executable launched through Proton.
    pub executable: PathBuf,
    /// Working directory for the process (defaults to the exe's parent).
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    /// Per-game Proton prefix (WINEPREFIX). Managed under the data dir.
    pub prefix: PathBuf,
    /// Proton build umu should use, e.g. "GE-Proton" or "UMU-Latest".
    #[serde(default = "default_proton")]
    pub proton: String,
    #[serde(default)]
    pub steam_mode: SteamMode,
    /// Steam-style launch options, e.g. `gamescope -f -- %command%` or
    /// `WINEDLLOVERRIDES="mss32=n,b" %command% -novid`. See [`crate::launch_options`].
    #[serde(default)]
    pub launch_options: String,
    /// Local artwork paths, populated by the artwork fetcher.
    #[serde(default)]
    pub artwork: Artwork,
    /// Whether `gbe_fork` has been deployed into the game dir.
    #[serde(default)]
    pub emu_deployed: bool,
    #[serde(default)]
    pub last_played: Option<i64>,
    #[serde(default)]
    pub play_seconds: u64,
}

fn default_proton() -> String {
    "GE-Proton".to_owned()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Artwork {
    pub cover: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub icon: Option<PathBuf>,
}

impl Game {
    pub fn new(title: impl Into<String>, executable: PathBuf, prefix: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            app_id: None,
            title: title.into(),
            executable,
            working_dir: None,
            prefix,
            proton: default_proton(),
            steam_mode: SteamMode::default(),
            launch_options: String::new(),
            artwork: Artwork::default(),
            emu_deployed: false,
            last_played: None,
            play_seconds: 0,
        }
    }

    /// Directory the executable lives in — where `gbe_fork` DLLs get deployed.
    #[must_use]
    pub fn game_dir(&self) -> PathBuf {
        self.working_dir.clone().unwrap_or_else(|| {
            self.executable
                .parent()
                .map(PathBuf::from)
                .unwrap_or_default()
        })
    }
}
