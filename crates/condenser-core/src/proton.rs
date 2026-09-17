//! Discovery of Proton builds already installed on the machine.
//!
//! umu accepts `PROTONPATH` as either an absolute path to a Proton build or a bare name
//! like `GE-Proton`, which it then tries to *download*. Passing a bare name on a machine
//! that is offline (or behind a GitHub rate limit) fails with a confusing
//! `Environment variable not set or is empty: PROTONPATH`, even though plenty of Proton
//! builds are installed locally. Resolving to a real path up front avoids that entirely.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A Proton build found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonBuild {
    /// Display name — the directory name, e.g. "Proton-GE Latest".
    pub name: String,
    pub path: PathBuf,
}

/// Marker file that identifies a directory as a Proton build.
const MARKER: &str = "proton";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Directories that may contain Proton builds.
fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(home) = home() else {
        return roots;
    };

    // User-installed compatibility tools.
    for rel in [
        ".local/share/Steam/compatibilitytools.d",
        ".steam/root/compatibilitytools.d",
        ".steam/steam/compatibilitytools.d",
        ".var/app/com.valvesoftware.Steam/data/Steam/compatibilitytools.d",
    ] {
        roots.push(home.join(rel));
    }

    // Valve's own Proton builds ship as ordinary apps inside each Steam library.
    for lib in steam_libraries(&home) {
        roots.push(lib.join("steamapps/common"));
    }
    roots
}

/// Steam library folders, parsed out of `libraryfolders.vdf`.
///
/// The VDF is scanned for `"path"  "..."` entries rather than fully parsed — the file
/// format is stable and this avoids a VDF dependency for one field.
fn steam_libraries(home: &Path) -> Vec<PathBuf> {
    let mut libs = Vec::new();
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
    ];
    for root in candidates {
        if !root.is_dir() {
            continue;
        }
        libs.push(root.clone());
        let vdf = root.join("steamapps/libraryfolders.vdf");
        let Ok(text) = std::fs::read_to_string(&vdf) else {
            continue;
        };
        libs.extend(parse_library_paths(&text));
    }
    libs.sort();
    libs.dedup();
    libs
}

/// Pull the `"path"` values out of a `libraryfolders.vdf`.
fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("\"path\"")?;
            let start = rest.find('"')? + 1;
            let rest = rest.get(start..)?;
            let end = rest.find('"')?;
            rest.get(..end).map(PathBuf::from)
        })
        .collect()
}

/// Is this directory a usable Proton build?
fn is_proton(dir: &Path) -> bool {
    dir.join(MARKER).is_file()
}

/// Every Proton build discoverable on this machine, sorted by name.
///
/// Paths are canonicalised before de-duplication: `~/.steam/root` and `~/.steam/steam`
/// are symlinks to the same Steam directory, so the same build is otherwise reported
/// once per alias.
#[must_use]
pub fn discover() -> Vec<ProtonBuild> {
    let mut found: Vec<ProtonBuild> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for root in search_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !is_proton(&path) {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            found.push(ProtonBuild {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: canonical,
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Resolve a configured Proton setting to a value umu can use.
///
/// Accepts an absolute path, the name of a discovered build, or a bare umu name such as
/// `GE-Proton` (returned unchanged, letting umu download it). A discovered build always
/// wins over a download, since it works offline.
#[must_use]
pub fn resolve(setting: &str) -> String {
    let as_path = Path::new(setting);
    if as_path.is_absolute() && as_path.is_dir() {
        return setting.to_owned();
    }
    let builds = discover();
    if let Some(exact) = builds.iter().find(|b| b.name == setting) {
        return exact.path.to_string_lossy().into_owned();
    }
    // Token match, so a stored "GE-Proton" picks up a local "Proton-GE Latest".
    if let Some(fuzzy) = builds.iter().find(|b| tokens_match(setting, &b.name)) {
        return fuzzy.path.to_string_lossy().into_owned();
    }
    // Nothing local matched: hand the name back and let umu try to fetch it.
    setting.to_owned()
}

/// Split a build name into lowercase alphanumeric tokens: "Proton-GE Latest" becomes
/// `["proton", "ge", "latest"]`.
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// True when every token of `needle` appears in `candidate`, so "GE-Proton" matches
/// "Proton-GE Latest" but not "Proton - Experimental".
fn tokens_match(needle: &str, candidate: &str) -> bool {
    let have = tokens(candidate);
    let want = tokens(needle);
    !want.is_empty() && want.iter().all(|t| have.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths_from_vdf() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path"		"/home/blu/.local/share/Steam"
        "label"		""
    }
    "1"
    {
        "path"		"/run/media/system/Data/Games/Steam"
    }
}
"#;
        let paths = parse_library_paths(vdf);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/blu/.local/share/Steam"),
                PathBuf::from("/run/media/system/Data/Games/Steam"),
            ]
        );
    }

    #[test]
    fn absolute_existing_path_is_passed_through() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().to_string_lossy().into_owned();
        assert_eq!(resolve(&p), p);
    }

    #[test]
    fn unknown_name_falls_back_to_umu_download() {
        // A name that cannot match anything local is handed to umu untouched.
        assert_eq!(resolve("Definitely-Not-Installed-9999"), "Definitely-Not-Installed-9999");
    }

    /// The default setting "GE-Proton" must find a locally installed "Proton-GE Latest",
    /// and must not match an unrelated Valve build.
    #[test]
    fn ge_proton_matches_local_proton_ge_builds() {
        assert!(tokens_match("GE-Proton", "Proton-GE Latest"));
        assert!(tokens_match("GE-Proton", "Proton-GE RTSP Latest"));
        assert!(!tokens_match("GE-Proton", "Proton - Experimental"));
        assert!(!tokens_match("GE-Proton", "Proton 9.0 (Beta)"));
        // An exact name still matches itself.
        assert!(tokens_match("Proton Hotfix", "Proton Hotfix"));
        // A more specific request must not match a less specific build.
        assert!(!tokens_match("Proton-GE RTSP Latest", "Proton-GE Latest"));
        assert!(!tokens_match("", "Proton-GE Latest"));
    }

    #[test]
    fn is_proton_requires_the_marker_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Proton-GE Latest");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_proton(&dir), "a bare directory is not a Proton build");
        std::fs::write(dir.join("proton"), b"#!/usr/bin/env python").unwrap();
        assert!(is_proton(&dir));
    }
}
