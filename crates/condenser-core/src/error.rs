use std::path::PathBuf;

/// Errors surfaced by the Condenser engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("game not found: {0}")]
    GameNotFound(uuid::Uuid),

    #[error("executable does not exist: {0}")]
    MissingExecutable(PathBuf),

    #[error("no Steamworks library found under {0} (expected steam_api.dll / steam_api64.dll / libsteam_api.so)")]
    NoSteamApi(PathBuf),

    #[error("gbe_fork runtime not installed; run Condenser setup first")]
    GbeNotInstalled,

    #[error("{name} is missing from {gbe_dir} — install the runtime for this platform")]
    MissingEmuLibrary {
        name: &'static str,
        gbe_dir: PathBuf,
    },

    #[error("umu-run not found on PATH or at a configured location")]
    UmuNotFound,

    #[error("launch failed with status {0}")]
    LaunchFailed(std::process::ExitStatus),

    #[error("umu failed to start the game ({marker}): {detail}")]
    UmuReportedFailure { marker: String, detail: String },

    #[error("could not check for gbe_fork updates: {0}")]
    UpdateCheckFailed(String),

    #[error("gbe_fork install failed: {0}")]
    InstallFailed(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
