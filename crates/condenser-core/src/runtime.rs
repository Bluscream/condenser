//! Tracking and installation of the `gbe_fork` runtime.
//!
//! Condenser never bundles `gbe_fork` (it releases every 2-4 weeks, so a bundled copy is
//! stale almost immediately). It is fetched here instead, and the installed release is
//! recorded so the UI can tell the user when they are behind.
//!
//! Everything — HTTP, archive extraction, manifest writing — is done in-process. An
//! earlier version shelled out to `curl`/`python3`/`7z`/`tar`, which needed five external
//! programs, could not be unit-tested, and required locating an installer script at
//! runtime (fragile once packaged).

use crate::config::EmulatorSource;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

const USER_AGENT: &str = concat!("condenser/", env!("CARGO_PKG_VERSION"));

/// Libraries we extract out of a release archive and deploy into games.
const WANTED: &[&str] = &[
    "steam_api.dll",
    "steam_api64.dll",
    "steamclient.dll",
    "steamclient64.dll",
    "libsteam_api.so",
];

/// Which platform's games a release archive serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Both,
}

impl Platform {
    const fn wants_windows(self) -> bool {
        matches!(self, Self::Windows | Self::Both)
    }
    const fn wants_linux(self) -> bool {
        matches!(self, Self::Linux | Self::Both)
    }
}

impl std::str::FromStr for Platform {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            "both" => Ok(Self::Both),
            other => Err(Error::InstallFailed(format!(
                "unknown platform {other:?} (expected windows, linux or both)"
            ))),
        }
    }
}

/// Recorded alongside the installed libraries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Upstream release tag, e.g. "release-2026_09_16_2".
    pub tag: String,
    /// Publish date of that release (YYYY-MM-DD).
    #[serde(default)]
    pub published: String,
    /// Unix timestamp of when we installed it.
    #[serde(default)]
    pub installed_at: i64,
    /// Library filenames we extracted.
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub source: String,
}

impl Manifest {
    #[must_use]
    pub fn path(gbe_dir: &Path) -> PathBuf {
        gbe_dir.join("condenser-manifest.json")
    }

    #[must_use]
    pub fn load(gbe_dir: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(Self::path(gbe_dir)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(&self, gbe_dir: &Path) -> Result<()> {
        std::fs::write(Self::path(gbe_dir), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    #[must_use]
    pub fn has_windows(&self) -> bool {
        self.assets.iter().any(|a| has_extension(a, "dll"))
    }

    #[must_use]
    pub fn has_linux(&self) -> bool {
        self.assets.iter().any(|a| has_extension(a, "so"))
    }

    /// Whole days since installation — truncation toward zero is the intent here,
    /// since the UI reports "N days ago".
    #[must_use]
    #[allow(clippy::integer_division, reason = "whole days is the desired unit")]
    pub fn age_days(&self) -> Option<i64> {
        if self.installed_at == 0 {
            return None;
        }
        Some((now_unix() - self.installed_at) / 86_400)
    }
}

/// Case-insensitive extension test, so an oddly-cased archive entry still counts.
fn has_extension(name: &str, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// What the UI shows in the runtime panel.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    pub installed: Option<Manifest>,
    pub latest_tag: Option<String>,
    pub update_available: bool,
}

/// Inspect the local install without touching the network.
#[must_use]
pub fn status(gbe_dir: &Path) -> RuntimeStatus {
    RuntimeStatus {
        installed: Manifest::load(gbe_dir),
        latest_tag: None,
        update_available: false,
    }
}

// --- GitHub release metadata ---------------------------------------------

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// Pick an asset by substring, preferring the lowest-sorting match so we take the
    /// older toolchain build (vs22 over vs26) for broader compatibility.
    fn pick(&self, substr: &str) -> Option<&Asset> {
        let mut matches: Vec<&Asset> = self
            .assets
            .iter()
            .filter(|a| a.name.contains(substr))
            .collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches.into_iter().next()
    }

    fn published_date(&self) -> String {
        self.published_at.chars().take(10).collect()
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Add a token when one is available — unauthenticated GitHub allows 60 req/h.
fn github_json(url: &str) -> Result<Release> {
    let mut req = agent().get(url).header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
    }
    let mut resp = req
        .call()
        .map_err(|e| Error::UpdateCheckFailed(e.to_string()))?;
    resp.body_mut()
        .read_json::<Release>()
        .map_err(|e| Error::UpdateCheckFailed(format!("malformed release JSON: {e}")))
}

fn fetch_release(src: &EmulatorSource, tag: Option<&str>) -> Result<Release> {
    let (base, repo) = (src.api_base.trim_end_matches('/'), &src.repo);
    let url = match tag {
        Some(tag) => format!("{base}/repos/{repo}/releases/tags/{tag}"),
        None => format!("{base}/repos/{repo}/releases/latest"),
    };
    github_json(&url)
}

/// Newest upstream release tag. Requires network.
pub fn latest_tag(src: &EmulatorSource) -> Result<String> {
    Ok(fetch_release(src, None)?.tag_name)
}

/// Local status plus an online freshness check.
pub fn check_for_update(gbe_dir: &Path, src: &EmulatorSource) -> Result<RuntimeStatus> {
    let installed = Manifest::load(gbe_dir);
    let latest = latest_tag(src)?;
    // Upstream tags are date-ordered (release-YYYY_MM_DD[_n]), so string inequality is
    // a correct "something newer exists" test.
    let update_available = installed.as_ref().is_none_or(|m| m.tag != latest);
    Ok(RuntimeStatus {
        installed,
        latest_tag: Some(latest),
        update_available,
    })
}

/// List recent releases as (tag, date) pairs, for pinning in the UI.
pub fn list_releases(src: &EmulatorSource, limit: usize) -> Result<Vec<(String, String)>> {
    let base = src.api_base.trim_end_matches('/');
    let repo = &src.repo;
    let url = format!("{base}/repos/{repo}/releases?per_page={limit}");
    let mut req = agent().get(&url).header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
    }
    let mut resp = req
        .call()
        .map_err(|e| Error::UpdateCheckFailed(e.to_string()))?;
    let releases: Vec<Release> = resp
        .body_mut()
        .read_json()
        .map_err(|e| Error::UpdateCheckFailed(format!("malformed release list: {e}")))?;
    Ok(releases
        .into_iter()
        .map(|r| {
            let date = r.published_date();
            (r.tag_name, date)
        })
        .collect())
}

// --- installation ---------------------------------------------------------

/// Progress callback: (stage description, bytes done, total bytes if known).
pub type Progress<'a> = dyn FnMut(&str, u64, Option<u64>) + 'a;

fn download(url: &str, dest: &Path, progress: &mut Progress<'_>) -> Result<()> {
    let mut resp = agent()
        .get(url)
        .call()
        .map_err(|e| Error::InstallFailed(format!("download failed: {e}")))?;

    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::io::BufWriter::new(std::fs::File::create(dest)?);
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        // `get(..n)` rather than `buf[..n]`: Read guarantees n <= buf.len(), but the
        // fallible form keeps the panic-free discipline the crate lints for.
        let chunk = buf.get(..n).ok_or_else(|| {
            Error::InstallFailed("reader returned more bytes than the buffer holds".into())
        })?;
        std::io::Write::write_all(&mut file, chunk)?;
        done = done.saturating_add(u64::try_from(n).unwrap_or(0));
        progress("downloading", done, total);
    }
    std::io::Write::flush(&mut file)?;
    Ok(())
}

/// Copy out the libraries we care about, flattening the archive's directory layout.
fn harvest(root: &Path, gbe_dir: &Path, installed: &mut Vec<String>) -> Result<()> {
    for name in WANTED {
        if installed.iter().any(|i| i == name) {
            continue;
        }
        if let Some(found) = find_named(root, name) {
            let dest = gbe_dir.join(name);
            // The destination may be locked read-only by a live deployment; removing
            // needs directory write permission only.
            let _ = std::fs::remove_file(&dest);
            std::fs::copy(&found, &dest)?;
            crate::gbe::lock_runtime_file(&dest);
            installed.push(name.to_string());
        }
    }
    Ok(())
}

fn find_named(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = entry.file_type().ok()?;
        if ft.is_dir() {
            dirs.push(path);
        } else if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            return Some(path);
        }
    }
    dirs.into_iter().find_map(|d| find_named(&d, name))
}

fn extract_7z(archive: &Path, dest: &Path) -> Result<()> {
    sevenz_rust2::decompress_file(archive, dest)
        .map_err(|e| Error::InstallFailed(format!("7z extraction failed: {e}")))
}

fn extract_tar_bz2(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest)
        .map_err(|e| Error::InstallFailed(format!("tar.bz2 extraction failed: {e}")))
}

/// Download and install the `gbe_fork` runtime.
///
/// `tag` pins a release; `None` takes the latest. Existing libraries are replaced even
/// when locked read-only by a live deployment.
pub fn install(
    gbe_dir: &Path,
    src: &EmulatorSource,
    tag: Option<&str>,
    platform: Platform,
    progress: &mut Progress<'_>,
) -> Result<Manifest> {
    std::fs::create_dir_all(gbe_dir)?;

    progress("resolving release", 0, None);
    let release = fetch_release(src, tag)?;

    // Resolve every asset up front as owned values, so the release metadata can be
    // consumed into the manifest afterwards.
    let tag = release.tag_name.clone();
    let published = release.published_date();
    let win = platform
        .wants_windows()
        .then(|| {
            release
                .pick(&src.windows_asset)
                .map(|a| (a.name.clone(), a.browser_download_url.clone()))
                .ok_or_else(|| Error::InstallFailed(format!("no Windows asset in {tag}")))
        })
        .transpose()?;
    let lin = platform
        .wants_linux()
        .then(|| {
            release
                .pick(&src.linux_asset)
                .map(|a| (a.name.clone(), a.browser_download_url.clone()))
                .ok_or_else(|| Error::InstallFailed(format!("no Linux asset in {tag}")))
        })
        .transpose()?;

    let tmp = tempdir(gbe_dir)?;
    let mut installed: Vec<String> = Vec::new();

    if let Some((name, url)) = win {
        let archive = tmp.join(&name);
        download(&url, &archive, progress)?;
        progress("extracting Windows libraries", 0, None);
        let out = tmp.join("win");
        extract_7z(&archive, &out)?;
        harvest(&out, gbe_dir, &mut installed)?;
    }

    if let Some((name, url)) = lin {
        let archive = tmp.join(&name);
        download(&url, &archive, progress)?;
        progress("extracting Linux libraries", 0, None);
        let out = tmp.join("linux");
        extract_tar_bz2(&archive, &out)?;
        harvest(&out, gbe_dir, &mut installed)?;
    }

    let _ = std::fs::remove_dir_all(&tmp);

    if installed.is_empty() {
        return Err(Error::InstallFailed(
            "no Steamworks libraries found in the release archives".into(),
        ));
    }

    installed.sort();
    let manifest = Manifest {
        tag,
        published,
        installed_at: now_unix(),
        assets: installed,
        source: src.repo.clone(),
    };
    manifest.save(gbe_dir)?;
    progress("done", 0, None);
    Ok(manifest)
}

/// Scratch directory beside the target, so extraction never crosses a filesystem.
fn tempdir(near: &Path) -> Result<PathBuf> {
    let dir = near.join(format!(".condenser-tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_manifest_reports_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let st = status(tmp.path());
        assert!(st.installed.is_none());
        assert!(!st.update_available);
    }

    #[test]
    fn manifest_roundtrip_and_platform_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Manifest {
            tag: "release-2026_09_16_2".into(),
            published: "2026-09-16".into(),
            installed_at: 1_700_000_000,
            assets: vec!["steam_api64.dll".into(), "libsteam_api.so".into()],
            source: String::new(),
        };
        m.save(tmp.path()).unwrap();

        let loaded = Manifest::load(tmp.path()).expect("manifest should load");
        assert_eq!(loaded.tag, "release-2026_09_16_2");
        assert!(loaded.has_windows());
        assert!(loaded.has_linux());
    }

    #[test]
    fn windows_only_install_is_detected_as_missing_linux() {
        let m = Manifest {
            tag: "t".into(),
            published: String::new(),
            installed_at: 0,
            assets: vec!["steam_api64.dll".into()],
            source: String::new(),
        };
        assert!(m.has_windows());
        assert!(!m.has_linux(), "native Linux games would have no .so to swap");
        assert!(m.age_days().is_none());
    }

    #[test]
    fn platform_parsing() {
        use std::str::FromStr;
        assert_eq!(Platform::from_str("windows").unwrap(), Platform::Windows);
        assert_eq!(Platform::from_str("both").unwrap(), Platform::Both);
        assert!(Platform::from_str("bogus").is_err());
    }

    /// Asset selection must prefer the vs22 build and tolerate unrelated assets.
    #[test]
    fn asset_selection_prefers_older_toolchain() {
        let release = Release {
            tag_name: "release-2026_09_16_2".into(),
            published_at: "2026-09-16T12:00:00Z".into(),
            assets: vec![
                asset("emu-win-release-vs26.7z"),
                asset("emu-win-release-vs22.7z"),
                asset("emu-linux-release.tar.bz2"),
                asset("migrate_gse-win.7z"),
            ],
        };
        assert_eq!(
            release.pick(&EmulatorSource::default().windows_asset).unwrap().name,
            "emu-win-release-vs22.7z"
        );
        assert_eq!(
            release.pick(&EmulatorSource::default().linux_asset).unwrap().name,
            "emu-linux-release.tar.bz2"
        );
        assert_eq!(release.published_date(), "2026-09-16");
        assert!(release.pick("nonexistent").is_none());
    }

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: format!("https://example.invalid/{name}"),
        }
    }

    /// `harvest()` must find libraries at any depth and flatten them.
    #[test]
    fn harvest_flattens_nested_libraries() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("extracted/release/steamclient_experimental");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("steam_api64.dll"), b"EMU").unwrap();
        std::fs::write(src.join("unrelated.txt"), b"x").unwrap();

        let gbe = tmp.path().join("gbe");
        std::fs::create_dir_all(&gbe).unwrap();

        let mut installed = Vec::new();
        harvest(&tmp.path().join("extracted"), &gbe, &mut installed).unwrap();

        assert_eq!(installed, vec!["steam_api64.dll"]);
        assert_eq!(std::fs::read(gbe.join("steam_api64.dll")).unwrap(), b"EMU");
        assert!(!gbe.join("unrelated.txt").exists());
    }

    /// Reinstalling over libraries locked by a live deployment must succeed.
    #[cfg(unix)]
    #[test]
    fn harvest_replaces_readonly_libraries() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("extracted");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("steam_api64.dll"), b"EMU-V2").unwrap();

        let gbe = tmp.path().join("gbe");
        std::fs::create_dir_all(&gbe).unwrap();
        let existing = gbe.join("steam_api64.dll");
        std::fs::write(&existing, b"EMU-V1").unwrap();
        crate::gbe::lock_runtime_file(&existing);

        let mut installed = Vec::new();
        harvest(&src, &gbe, &mut installed).unwrap();

        assert_eq!(std::fs::read(&existing).unwrap(), b"EMU-V2".to_vec());
        assert_eq!(
            std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o444,
            "freshly installed libraries should be locked again"
        );
    }
}
