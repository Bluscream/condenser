//! Thin orchestration layer over `umu-run` (Open-Wine-Components/umu-launcher).
//!
//! umu provides Proton + the Steam Linux Runtime container, so we never need the
//! Steam client itself — we just hand it an executable and a prefix.

use crate::config::Settings;
use crate::error::{Error, Result};
use crate::launch_options;
use crate::model::Game;
use std::path::PathBuf;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Resolve the umu-run binary: explicit setting, then PATH, then common install spots.
pub fn resolve_umu(settings: &Settings) -> Result<PathBuf> {
    if let Some(p) = &settings.umu.run_path {
        if p.exists() {
            return Ok(p.clone());
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("umu-run");
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    for cand in [
        "/usr/bin/umu-run",
        "/usr/local/bin/umu-run",
        "/app/bin/umu-run",
    ] {
        let p = PathBuf::from(cand);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(Error::UmuNotFound)
}

/// Build the fully-configured launch command for a game without running it.
/// Kept separate from `launch` so the UI can preview it and tests can assert on it.
pub fn build_command(game: &Game, settings: &Settings) -> Result<Command> {
    if !game.executable.exists() {
        return Err(Error::MissingExecutable(game.executable.clone()));
    }
    let umu = resolve_umu(settings)?;
    let opts = launch_options::parse(&game.launch_options);

    // `%command%` stands for `umu-run <exe>`. Anything before it wraps that command
    // (gamescope, mangohud, …); anything after becomes extra game arguments.
    let mut cmd = match opts.wrapper.split_first() {
        Some((program, args)) => {
            let mut cmd = Command::new(program);
            cmd.args(args);
            cmd.arg(&umu);
            cmd
        }
        None => Command::new(&umu),
    };
    cmd.arg(&game.executable);
    cmd.args(&opts.extra_args);
    cmd.current_dir(game.game_dir());

    // GAMEID drives umu's protonfixes lookup. Using the real Steam AppID gets us the
    // community fixes for that title; otherwise we fall back to a generic id.
    let gameid = match game.app_id {
        Some(id) => id.to_string(),
        None => "umu-default".to_owned(),
    };
    cmd.env("GAMEID", gameid);
    // STORE=none tells protonfixes this isn't an EGS/GOG install.
    cmd.env("STORE", "none");
    // Resolve to an absolute path where possible. A bare name makes umu try to
    // *download* that Proton build, which fails on an offline or rate-limited machine
    // even when suitable builds are already installed.
    cmd.env("PROTONPATH", crate::proton::resolve(&game.proton));
    cmd.env("WINEPREFIX", &game.prefix);

    // Applied last so a user's launch options can override anything above.
    for (key, value) in &opts.env {
        cmd.env(key, value);
    }
    Ok(cmd)
}

/// Markers umu prints when it fails to start the game.
///
/// `umu-run` exits 0 even after a fatal error — a bad `PROTONPATH` produces a Python
/// traceback and still returns success — so the exit status alone would record a
/// phantom play session. stderr is inspected as well.
const FAILURE_MARKERS: &[&str] = &[
    "Traceback (most recent call last)",
    "FileNotFoundError",
    "ERROR: Environment variable not set or is empty",
];

/// Launch the game and wait for it to exit, returning the wall-clock playtime.
///
/// stderr is teed: it is echoed so the user still sees umu's progress, and retained so
/// a silent failure can be detected.
pub fn launch_blocking(game: &Game, settings: &Settings) -> Result<std::time::Duration> {
    let mut cmd = build_command(game, settings)?;
    cmd.stderr(Stdio::piped());

    let start = std::time::Instant::now();
    let mut child = cmd.spawn()?;

    let stderr = child.stderr.take();
    let captured = stderr.map_or_else(String::new, |stderr| {
        let mut text = String::new();
        for line in BufRead::lines(BufReader::new(stderr)).map_while(std::result::Result::ok) {
            eprintln!("{line}");
            text.push_str(&line);
            text.push('\n');
        }
        text
    });

    let status = child.wait()?;
    if !status.success() {
        return Err(Error::LaunchFailed(status));
    }
    if let Some(marker) = FAILURE_MARKERS.iter().find(|m| captured.contains(**m)) {
        return Err(Error::UmuReportedFailure {
            marker: (*marker).to_owned(),
            detail: last_meaningful_line(&captured),
        });
    }
    Ok(start.elapsed())
}

/// The most useful line to show the user: umu's final ERROR line, else the last line.
fn last_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|l| l.contains("ERROR:") || l.contains("Error:"))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_exe_is_rejected() {
        let game = Game::new(
            "Nope",
            PathBuf::from("/definitely/not/here.exe"),
            PathBuf::from("/tmp/pfx"),
        );
        assert!(matches!(
            build_command(&game, &Settings::default()),
            Err(Error::MissingExecutable(_))
        ));
    }

    /// A gamescope wrapper must become the program actually executed, with umu-run
    /// and the game passed through to it.
    #[test]
    fn launch_options_wrapper_becomes_the_program() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        std::fs::write(&exe, b"x").unwrap();
        let umu = tmp.path().join("umu-run");
        std::fs::write(&umu, b"#!/bin/sh").unwrap();

        let mut settings = Settings::default();
        settings.umu.run_path = Some(umu.clone());

        let mut game = Game::new("G", exe.clone(), tmp.path().join("pfx"));
        game.launch_options =
            r"DXVK_HUD=fps gamescope -f -- %command% -novid".to_owned();

        let cmd = build_command(&game, &settings).unwrap();
        assert_eq!(cmd.get_program(), "gamescope");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec![
                "-f".to_owned(),
                "--".to_owned(),
                umu.to_string_lossy().into_owned(),
                exe.to_string_lossy().into_owned(),
                "-novid".to_owned(),
            ]
        );
        assert!(
            cmd.get_envs().any(|(k, v)| k == "DXVK_HUD"
                && v.is_some_and(|v| v == "fps")),
            "environment assignment from launch options should be applied"
        );
    }

    /// With no launch options the program is umu-run itself.
    #[test]
    fn without_launch_options_umu_is_the_program() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        std::fs::write(&exe, b"x").unwrap();
        let umu = tmp.path().join("umu-run");
        std::fs::write(&umu, b"#!/bin/sh").unwrap();

        let mut settings = Settings::default();
        settings.umu.run_path = Some(umu.clone());
        let game = Game::new("G", exe, tmp.path().join("pfx"));

        let cmd = build_command(&game, &settings).unwrap();
        assert_eq!(cmd.get_program(), umu.as_os_str());
    }
}
