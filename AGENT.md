# AGENT.md — Condenser build & debugging notes

Operational notes for working on this repo. Everything here was hit and verified in
practice; none of it is speculative.

---

## Host context

This project is developed on **Bazzite (Fedora-based, immutable)**. `webkit2gtk-4.1`
cannot be installed on the host, so the Tauri shell is compiled inside the **`build-box`**
Distrobox container (Debian 13).

```bash
# one-time container setup
distrobox enter build-box -- bash -c "sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libxdo-dev libssl-dev"
```

`distrobox enter` resets cwd to `$HOME`, so always chain `cd` inside the same command
string:

```bash
distrobox enter build-box -- bash -c "cd /run/media/system/Data/Projects/condenser && cargo tauri build"
```

**Layer split that makes this workable:** `crates/condenser-core` has no GUI dependencies,
so it builds and tests on the *host* directly. Only `src-tauri` needs the container.

```bash
cargo test -p condenser-core        # runs on the host, no container needed
```

---

## Gotcha 1 — `cargo build` needs `--features custom-protocol`

**Symptom:** the window opens with the correct title and icon, but the content is blank.
With the CSP removed you instead see *"Could not connect to localhost: Connection
refused"*.

**Cause:** Tauri only serves the embedded frontend when the `custom-protocol` feature is
on. Without it the binary falls back to `devUrl` (`http://localhost:1420`), where nothing
is listening. `cargo tauri build` injects this feature automatically — plain `cargo build`
does not.

```bash
# wrong — blank window
cargo build -p condenser --release

# right
cargo build -p condenser --release --features custom-protocol
```

**Debugging tip:** a blank *white* page means the document never loaded at all (our CSS
paints the body dark). If you see the dark background but no content, that is a JS
problem instead — a completely different bug.

**Dead end worth recording:** the embedded assets are stored in a packed, newline-free
string blob, so `strings binary | grep '^/src/main.js$'` finds nothing even when the asset
*is* embedded. Do not conclude assets are missing from that. Grep without anchors.

---

## Gotcha 2 — AppImage aborts with `EGL_BAD_PARAMETER` on Fedora hosts

**Symptom:** the AppImage window is blank; stderr shows

```
Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...
```

The plain binary works fine on the same machine — this is AppImage-specific.

**Cause:** upstream Tauri bug
[tauri-apps/tauri#15976](https://github.com/tauri-apps/tauri/issues/15976). The bundler
over-bundles display-stack libraries (`libwayland-client`, `libwayland-egl`,
`libxkbcommon`, `libxcb*`) and puts them ahead of the host copies on `LD_LIBRARY_PATH`.
The host's newer Mesa then negotiates EGL against the container-era `libwayland-client`
and `eglGetDisplay` fails. Building in Debian and running on Fedora is exactly the
mismatch that triggers it.

**Verified workaround** — preload the host's wayland client:

```bash
LD_PRELOAD=/usr/lib64/libwayland-client.so.0 ./Condenser_0.1.0_amd64.AppImage
```

Things that do **not** help (tested): `WEBKIT_DISABLE_DMABUF_RENDERER=1`,
`WEBKIT_DISABLE_COMPOSITING_MODE=1`, `LIBGL_ALWAYS_SOFTWARE=1`. The abort happens before
WebKit's renderer selection, so those knobs never come into play.

**Proper fixes** (not yet applied): strip the offending libs from the AppImage and repack,
or try the experimental `TAURI_BUNDLER_NEW_APPIMAGE_FORMAT` bundler, which isolates
graphics drivers from bundled libraries.

---

## Lints

The workspace runs clippy `pedantic` + `nursery` with panic discipline as hard errors:
`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `integer_division` and
`too_many_lines` are all **deny**. `unsafe_code` is forbidden outright.

```bash
cargo clippy --workspace --all-targets --features condenser/custom-protocol
```

- **Function length** is capped at 100 lines by `too_many_lines`
  (`too-many-lines-threshold` in `clippy.toml`).
- **File length** is capped at 1000 lines by `crates/condenser-core/tests/source_limits.rs`,
  since clippy has no file-length lint. It fails in the normal `cargo test` run.
- Tests may `unwrap`/`expect`/`panic` freely — `clippy.toml` sets the `*-in-tests`
  allowances so assertions stay readable.

Deliberate exceptions, all scoped and justified in place:

| Lint | Where | Why |
| --- | --- | --- |
| `option_if_let_else` | workspace | Nursery lint that rewrites readable `if let/else` into nested closures. |
| `integer_division` | `Manifest::age_days` | Whole days is the intended unit. |
| `needless_pass_by_value`, `significant_drop_tightening` | `src-tauri` only | `#[tauri::command]` requires owned arguments and each handler holds its `MutexGuard` by design. |

The engine crate keeps the strict defaults; only the Tauri shell relaxes anything.

## No shell scripts in the install path

The runtime fetcher was originally `scripts/fetch-gbe.sh`. It was replaced with
`runtime::install` in Rust (`ureq` + `sevenz-rust2` + `tar`/`bzip2`) because the shell
version:

- needed five external programs (`curl`, `python3`, `7z`, `tar`, `bzip2`);
- could not be unit-tested, so the install path had zero coverage;
- had to be *located* at runtime, which would have broken once packaged;
- produced three of the four bugs hit while building it — a `set -e` abort from a
  trailing failed test that silently skipped the manifest write, and two rounds of
  Python-inside-single-quoted-shell escaping errors.

Keep install logic in Rust. The cost was about 3 MB of binary (17 MB to 20 MB).

## umu quirks worth remembering

Both were found by actually launching a game, not by reading docs:

* **`umu-run` exits 0 even after a fatal error.** A bad `PROTONPATH` prints a Python
  traceback and still returns success, so exit status alone records a phantom play
  session. `umu::launch_blocking` tees stderr and fails on `FAILURE_MARKERS`.
* **`PROTONPATH` must be a path, not a name.** A bare `GE-Proton` makes umu try to
  *download* that build; on a rate-limited or offline machine it then fails with
  `Environment variable not set or is empty: PROTONPATH` despite local builds existing.
  `proton::resolve` maps the setting to an absolute path first.

`proton::discover` canonicalises before de-duplicating: `~/.steam/root` and
`~/.steam/steam` are symlinks to the same directory, so builds otherwise appear 3x.

## Identifying DRM-wrapped executables

`gbe_fork` replaces the Steamworks API; it cannot help with Steam CEG. A CEG-wrapped
executable has a `.bind` PE section, which is a reliable pre-flight check:

| Executable | `.bind` | Result |
| --- | --- | --- |
| `iw3sp.exe` (CoD4) | yes | quits immediately — CEG needs Steam |
| `KisakCOD-sp.exe` (source port) | no | runs |
| `left4dead2.exe` | no | runs |

Worth surfacing in the UI eventually: warn at add-game time rather than letting the user
wonder why a game exits after eight seconds.

## One binary, two modes

`main()` dispatches on `std::env::args()`: no arguments opens the Tauri window, any
argument routes into `cli::run`. There is deliberately no second binary — the CLI and GUI
share one `Engine`, so behaviour cannot drift between them.

Consequence worth knowing: the binary links webkit even in CLI mode, so `condenser list`
still needs `libwebkit2gtk` present at runtime. That is the cost of the single-binary
requirement.

CLI argument parsing is hand-rolled in `cli.rs` (`split_flags`). Two rules matter:

* a bare `--` ends flag parsing, so values containing dashes survive;
* `set-options` bypasses flag parsing entirely and joins its arguments verbatim —
  launch options legitimately contain `--`, `-flags` and `%command%`, every one of which
  a flag parser would swallow. This was a real bug found by running the command, not a
  hypothetical.

## Dark native form controls

WebKitGTK renders `<select>` with the platform's **light** theme unless the page declares
its colour scheme — closed dropdowns looked washed out while their open option lists were
correctly dark. The fix is `color-scheme: dark` on `:root`, plus `appearance: none` and a
custom chevron so the control matches the panel rather than the platform.

## Emulator config is layered, and one key was wrong in 1.0.0

`emu_config.rs` stores settings flat, keyed by `user::general::account_name`, so merging
is trivial and no part of gbe_fork's schema has to be modelled. The section prefix selects
the INI file. Global lives in `Settings`, per-game on `Game`, and `deploy` writes
`global.merged_with(&game)` into `steam_settings/`.

Two things worth not re-breaking:

* `write_to` **deletes** a config file that no longer has entries. Without that, removing
  the last `main::` key would leave a stale `configs.main.ini` applying forever.
* 1.0.0 wrote `force_account_name.txt`, a **Goldberg-era** setting. Current gbe_fork does
  not read it — verified against `dll/settings_parser.cpp`, which takes `account_name`
  from `[user::general]` in `configs.user.ini`. `deploy` now deletes any stale copy.
  When touching emulator settings, check the parser rather than trusting Goldberg docs.

## The emulator settings widget

`createEmuConfigWidget({ mount, game })` in `frontend/src/main.js` builds its own DOM and
is instantiated twice — once with `game: null` (global, in the Runtime panel) and once
per selected game (`emuGameConfig.setGame(id)` when the drawer opens). The `game`
variable is the *only* difference between the two; there is deliberately no duplicated
markup in `index.html`, just two empty mount divs.

Behaviour that matters:

* rows are grouped by `<file>::<section>` and show only the leaf key, since the group
  header already carries the rest;
* an inherited row is dimmed with a `global` badge — **editing it creates an override**
  at that layer, which is the main interaction;
* an overridden row gets ↺ (revert to global) per-game, or ✕ (delete) when global;
* clearing an inline value removes the entry rather than storing an empty string.

## Testing the GUI

`mcp__linux-gui` drives the app. Two practical notes:

- **Pointer drift.** Clicks land 30–60 px off the requested coordinate in this
  environment. Use OCR targeting (`text: "Check for updates"`) rather than raw x/y —
  it resolves the element center and is far more reliable.
- **Toasts auto-hide after 4 s.** Use `settle_ms` under ~1500 or you will screenshot an
  empty overlay and wrongly conclude the click did nothing.
- **Kill stale instances** (`pkill -f Condenser`) before relaunching; leftover windows make
  `find_window` screenshot the wrong one.

---

## Manual end-to-end check

```bash
# 1. engine tests (host)
cargo test -p condenser-core

# 2. fetch the emulator and confirm the manifest lands
./target/release/condenser runtime install --platform both
./target/release/condenser runtime status

# 3. build + run (container build, host run)
distrobox enter build-box -- bash -c "cd $PWD && cargo build -p condenser --release --features custom-protocol"
./target/release/condenser list     # CLI
./target/release/condenser          # GUI
```

In the app, **⚙ Runtime** should show the installed tag, its age, and the platforms
covered; **Check for updates** should toast either "Runtime is up to date" or the newer
tag.

---

## Status of verification

| Area | State |
| --- | --- |
| Engine unit tests | ✅ 31 passing + file-length check |
| Clippy (workspace, all targets) | ✅ zero findings |
| Emulator fetch against live upstream | ✅ both platforms, manifest written |
| Update check (UI → IPC → GitHub) | ✅ "Runtime is up to date" |
| GUI | ✅ library, details, runtime, sources all render |
| CLI | ✅ every UI action reachable headless |
| **Game launch, Steam client closed** | ✅ **L4D2 reached its main menu** |
| Revert after deployment | ✅ hashes byte-identical, no leftovers |
| AppImage on Fedora hosts | ⚠️ needs `LD_PRELOAD` (upstream tauri#15976) |
| `generate_emu_config` / ColdClientLoader / artwork | ❌ not implemented |
