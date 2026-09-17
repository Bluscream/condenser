//! Deployment of the `gbe_fork` Steamworks emulator into a game directory.
//!
//! This does NOT bypass Steam DRM (`SteamStub`/CEG) — it only replaces the Steamworks
//! API, so games you own can run offline or on a LAN without the Steam client.
//!
//! Upstream: <https://github.com/Detanup01/gbe_fork>

use crate::error::{Error, Result};
use crate::model::{Game, SteamMode};
use crate::EmuConfig;
use std::fs;
use std::path::{Path, PathBuf};

/// Names of the vendored emulator libraries inside `gbe_dir`.
const EMU_WIN64: &str = "steam_api64.dll";
const EMU_WIN32: &str = "steam_api.dll";
const EMU_LINUX: &str = "libsteam_api.so";

/// Suffix used when backing up a game's original Steamworks library.
const BACKUP_SUFFIX: &str = ".condenser-orig";

/// Mode applied to files that must not be written while a deployment is live.
const MODE_READONLY: u32 = 0o444;
/// Mode restored when a file is handed back to the game.
const MODE_NORMAL: u32 = 0o644;

/// One discovered Steamworks library slot inside a game directory.
#[derive(Debug, Clone)]
pub struct SteamApiSlot {
    pub original: PathBuf,
    pub emu_source_name: &'static str,
}

/// Locate every `steam_api` library shipped with the game (there can be both 32/64-bit).
pub fn find_steam_api(game_dir: &Path) -> Result<Vec<SteamApiSlot>> {
    let mut slots = Vec::new();
    walk(game_dir, &mut |p| {
        match p.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.eq_ignore_ascii_case(EMU_WIN64) => slots.push(SteamApiSlot {
                original: p.to_path_buf(),
                emu_source_name: EMU_WIN64,
            }),
            Some(n) if n.eq_ignore_ascii_case(EMU_WIN32) => slots.push(SteamApiSlot {
                original: p.to_path_buf(),
                emu_source_name: EMU_WIN32,
            }),
            Some(n) if n.eq_ignore_ascii_case(EMU_LINUX) => slots.push(SteamApiSlot {
                original: p.to_path_buf(),
                emu_source_name: EMU_LINUX,
            }),
            _ => {}
        }
    })?;
    if slots.is_empty() {
        return Err(Error::NoSteamApi(game_dir.to_path_buf()));
    }
    Ok(slots)
}

/// Deploy `gbe_fork` into a game.
///
/// Backs up the originals, links in the emulator libraries, and writes the
/// `steam_settings/` folder (at minimum, the `AppID`). Idempotent — re-running refreshes
/// the emulator libraries but never clobbers an existing backup.
pub fn deploy(game: &Game, gbe_dir: &Path, config: &EmuConfig) -> Result<()> {
    if game.steam_mode == SteamMode::None {
        return Ok(());
    }
    if !gbe_dir.join(EMU_WIN64).exists() && !gbe_dir.join(EMU_LINUX).exists() {
        return Err(Error::GbeNotInstalled);
    }

    let game_dir = game.game_dir();
    for slot in find_steam_api(&game_dir)? {
        let backup = with_suffix(&slot.original, BACKUP_SUFFIX);
        let emu = gbe_dir.join(slot.emu_source_name);
        // Fail loudly rather than silently leaving the original in place — e.g. a
        // native Linux game when only the Windows archive was fetched.
        if !emu.exists() {
            return Err(Error::MissingEmuLibrary {
                name: slot.emu_source_name,
                gbe_dir: gbe_dir.to_path_buf(),
            });
        }
        // Absolute target, so the link resolves from wherever the game lives.
        let emu = fs::canonicalize(&emu)?;

        if !backup.exists() {
            // First deployment: *move* the original aside instead of copying it. The
            // bytes are preserved exactly and nothing is duplicated on disk.
            move_file(&slot.original, &backup)?;
        }
        // The backup is the only copy of the game's real library — keep it read-only
        // so a stray updater cannot clobber it either.
        set_mode(&backup, MODE_READONLY);

        // Critical for the symlink strategy: a Steam update or file validation writing
        // to the game's steam_api path would follow our link and corrupt the *shared*
        // library for every deployed game. Read-only makes that write fail instead.
        set_mode(&emu, MODE_READONLY);

        // On a re-deploy the current file is our own link/copy, never the original —
        // the backup above is the only authority for the game's real library.
        link_or_copy(&emu, &slot.original)?;
        write_steam_settings(slot.original.parent().unwrap_or(&game_dir), game, config)?;
    }
    Ok(())
}

/// Point `dest` at `src` with a symlink, so the (8-22 MB) emulator libraries exist
/// once on disk no matter how many games are deployed. Falls back to a real copy when
/// the game library cannot host symlinks — e.g. an NTFS or exFAT mount.
///
/// Returns true when a symlink was used.
fn link_or_copy(src: &Path, dest: &Path) -> Result<bool> {
    // symlink_metadata so a dangling link is still detected and cleared.
    if dest.symlink_metadata().is_ok() {
        fs::remove_file(dest)?;
    }
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(src, dest).is_ok() {
            return Ok(true);
        }
    }
    fs::copy(src, dest)?;
    Ok(false)
}

/// Best-effort permission change. Never fatal: a game library on NTFS/exFAT has no
/// meaningful Unix mode, and failing to lock a file should not block a deployment.
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(mode);
            let _ = fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// Lock a freshly installed runtime library so writes through a game's symlink fail.
pub fn lock_runtime_file(path: &Path) {
    set_mode(path, MODE_READONLY);
}

/// Make the shared runtime libraries writable again so they can be replaced.
///
/// `deploy` locks them to 0444 to protect against writes through a game's symlink, which
/// also means a runtime update cannot overwrite them in place. Call this first.
pub fn unlock_runtime(gbe_dir: &Path) -> Result<()> {
    if !gbe_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(gbe_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            set_mode(&entry.path(), MODE_NORMAL);
        }
    }
    Ok(())
}

/// Rename when possible, falling back to copy+remove across filesystem boundaries.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)?;
    fs::remove_file(from)?;
    Ok(())
}

/// Restore the game's original Steamworks libraries and remove our injected settings.
pub fn revert(game: &Game) -> Result<()> {
    let game_dir = game.game_dir();
    walk(&game_dir, &mut |_| {})?; // ensure dir is readable
    for slot in find_steam_api(&game_dir).unwrap_or_default() {
        let backup = with_suffix(&slot.original, BACKUP_SUFFIX);
        if backup.exists() {
            // Drop our symlink (or copy) first, then move the original back into
            // place — again a rename, so no data is rewritten.
            if slot.original.symlink_metadata().is_ok() {
                fs::remove_file(&slot.original)?;
            }
            move_file(&backup, &slot.original)?;
            // Hand the library back to the game writable, so its own updater can
            // replace it normally once we are out of the way.
            set_mode(&slot.original, MODE_NORMAL);
        }
        let settings = slot
            .original
            .parent()
            .unwrap_or(&game_dir)
            .join("steam_settings");
        if settings.exists() {
            let _ = fs::remove_dir_all(&settings);
        }
    }
    Ok(())
}

/// Write `steam_settings/` next to the emulator library: the `AppID` plus the merged
/// global and per-game configuration.
fn write_steam_settings(dir: &Path, game: &Game, config: &EmuConfig) -> Result<()> {
    let settings = dir.join("steam_settings");
    fs::create_dir_all(&settings)?;
    if let Some(app_id) = game.app_id {
        fs::write(settings.join("steam_appid.txt"), app_id.to_string())?;
    }
    config.write_to(&settings)?;

    // Goldberg read the account name from this file; current gbe_fork does not look at
    // it at all (it uses `account_name` in configs.user.ini). Remove any copy an older
    // Condenser left behind so it cannot mislead.
    let _ = fs::remove_file(settings.join("force_account_name.txt"));
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Depth-limited recursive walk, skipping the prefix/backup noise.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // file_type() does not follow symlinks: a symlinked directory must not be
        // descended into (recursion risk), and our own symlinked libraries must still
        // be reported as files so revert can find them.
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&path, f)?;
        } else {
            f(&path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Game;

    #[test]
    fn deploy_backs_up_and_swaps() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("steam_api64.dll"), b"ORIGINAL").unwrap();

        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU").unwrap();

        let mut game = Game::new("Test", game_dir.join("game.exe"), tmp.path().join("pfx"));
        game.app_id = Some(480);

        deploy(&game, &gbe, &EmuConfig::default()).unwrap();
        // The deployed library is a symlink into the shared gbe dir, not a copy.
        let deployed = game_dir.join("steam_api64.dll");
        assert!(
            deployed.symlink_metadata().unwrap().file_type().is_symlink(),
            "expected a symlink to avoid duplicating the emulator library"
        );
        assert_eq!(fs::read(&deployed).unwrap(), b"EMU".to_vec());
        assert_eq!(
            fs::read(game_dir.join("steam_api64.dll.condenser-orig")).unwrap(),
            b"ORIGINAL".to_vec()
        );
        assert_eq!(
            fs::read_to_string(game_dir.join("steam_settings/steam_appid.txt")).unwrap(),
            "480"
        );

        revert(&game).unwrap();
        let restored = game_dir.join("steam_api64.dll");
        assert_eq!(fs::read(&restored).unwrap(), b"ORIGINAL".to_vec());
        assert!(
            !restored.symlink_metadata().unwrap().file_type().is_symlink(),
            "revert must leave a real file, not a link"
        );
        assert!(!game_dir.join("steam_api64.dll.condenser-orig").exists());
        assert!(!game_dir.join("steam_settings").exists());
    }

    /// Re-deploying (e.g. after a runtime update) must refresh the link without ever
    /// overwriting the backup with our own artifact — that would destroy the only
    /// copy of the game's real library.
    #[test]
    fn redeploy_preserves_the_original_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("steam_api64.dll"), b"ORIGINAL").unwrap();

        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU-V1").unwrap();

        let game = Game::new("Test", game_dir.join("game.exe"), tmp.path().join("pfx"));
        deploy(&game, &gbe, &EmuConfig::default()).unwrap();

        // Simulate a gbe_fork update, then re-deploy. Unlocking first is what
        // `runtime::install` does — deploy leaves the library read-only.
        unlock_runtime(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU-V2").unwrap();
        deploy(&game, &gbe, &EmuConfig::default()).unwrap();

        assert_eq!(
            fs::read(game_dir.join("steam_api64.dll")).unwrap(),
            b"EMU-V2".to_vec(),
            "symlink should expose the updated library"
        );
        assert_eq!(
            fs::read(game_dir.join("steam_api64.dll.condenser-orig")).unwrap(),
            b"ORIGINAL".to_vec(),
            "backup must still hold the game's real library"
        );

        revert(&game).unwrap();
        assert_eq!(
            fs::read(game_dir.join("steam_api64.dll")).unwrap(),
            b"ORIGINAL".to_vec()
        );
    }

    /// The point of locking: a Steam update writing through a game's symlink must not
    /// be able to corrupt the shared runtime library.
    #[cfg(unix)]
    #[test]
    fn external_write_through_symlink_is_blocked() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("steam_api64.dll"), b"ORIGINAL").unwrap();

        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU").unwrap();

        let game = Game::new("Test", game_dir.join("game.exe"), tmp.path().join("pfx"));
        deploy(&game, &gbe, &EmuConfig::default()).unwrap();

        let shared = gbe.join("steam_api64.dll");
        assert_eq!(
            fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
            MODE_READONLY,
            "shared library must be read-only while deployed"
        );

        // Simulate a Steam update writing to the game's library path. It follows the
        // symlink to the shared file, which must reject the write.
        let res = fs::OpenOptions::new()
            .write(true)
            .open(game_dir.join("steam_api64.dll"));
        assert!(
            res.is_err(),
            "write through the symlink should have been denied"
        );
        assert_eq!(
            fs::read(&shared).unwrap(),
            b"EMU".to_vec(),
            "shared library must be intact"
        );

        // The backup holding the game's real library is protected too.
        let backup = game_dir.join("steam_api64.dll.condenser-orig");
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            MODE_READONLY
        );

        // After revert the game gets a writable file back so its own updater works.
        revert(&game).unwrap();
        let restored = game_dir.join("steam_api64.dll");
        assert_eq!(
            fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
            MODE_NORMAL
        );
        assert!(fs::OpenOptions::new().write(true).open(&restored).is_ok());
    }

    /// A runtime update must be able to replace libraries that deploy locked.
    #[cfg(unix)]
    #[test]
    fn unlock_runtime_allows_replacing_locked_libraries() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        let lib = gbe.join("steam_api64.dll");
        fs::write(&lib, b"EMU").unwrap();
        set_mode(&lib, MODE_READONLY);

        unlock_runtime(&gbe).unwrap();
        assert_eq!(
            fs::metadata(&lib).unwrap().permissions().mode() & 0o777,
            MODE_NORMAL
        );
        fs::write(&lib, b"EMU-V2").expect("unlocked library should be writable");
    }

    /// Two games sharing one emulator library must not each carry their own copy.
    #[test]
    fn multiple_games_share_one_library_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU").unwrap();
        let emu_canonical = fs::canonicalize(gbe.join("steam_api64.dll")).unwrap();

        for name in ["game_a", "game_b"] {
            let dir = tmp.path().join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("steam_api64.dll"), b"ORIGINAL").unwrap();
            let game = Game::new(name, dir.join("g.exe"), tmp.path().join("pfx"));
            deploy(&game, &gbe, &EmuConfig::default()).unwrap();

            let link = dir.join("steam_api64.dll");
            assert_eq!(
                fs::read_link(&link).unwrap(),
                emu_canonical,
                "{name} should link to the shared library"
            );
        }
    }

    /// A native Linux game when only the Windows archive was fetched must error,
    /// not quietly leave the game's original library in place.
    #[test]
    fn missing_platform_library_errors_instead_of_silently_skipping() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("libsteam_api.so"), b"ORIGINAL").unwrap();

        // gbe dir has only the Windows library.
        let gbe = tmp.path().join("gbe");
        fs::create_dir_all(&gbe).unwrap();
        fs::write(gbe.join("steam_api64.dll"), b"EMU").unwrap();

        let game = Game::new("Linux Game", game_dir.join("game"), tmp.path().join("pfx"));
        let err = deploy(&game, &gbe, &EmuConfig::default()).unwrap_err();
        assert!(
            matches!(err, Error::MissingEmuLibrary { name, .. } if name == "libsteam_api.so"),
            "expected MissingEmuLibrary, got {err:?}"
        );
        // Original must be untouched.
        assert_eq!(
            fs::read(game_dir.join("libsteam_api.so")).unwrap(),
            b"ORIGINAL".to_vec()
        );
    }
}
