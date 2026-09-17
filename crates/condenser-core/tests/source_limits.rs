//! Structural limits on the source itself.
//!
//! Clippy caps function length (`too_many_lines`, threshold in `clippy.toml`) but has no
//! file-length lint, so that half is enforced here. Keeping it as a test means it fails
//! in the normal `cargo test` run rather than depending on anyone remembering a script.

use std::path::{Path, PathBuf};

/// Maximum lines permitted in a single source file.
const MAX_LINES_PER_FILE: usize = 1000;

fn workspace_root() -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR is crates/condenser-core; the workspace is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            // Build output and VCS metadata are not our source.
            if name != "target" && name != ".git" {
                rust_sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_file_exceeds_the_line_limit() {
    let root = workspace_root().expect("workspace root should be two levels above the crate");
    let mut files = Vec::new();
    rust_sources(&root, &mut files);

    assert!(
        !files.is_empty(),
        "found no Rust sources under {} — the walk is broken",
        root.display()
    );

    let mut offenders: Vec<(PathBuf, usize)> = files
        .into_iter()
        .filter_map(|path| {
            let lines = std::fs::read_to_string(&path).ok()?.lines().count();
            (lines > MAX_LINES_PER_FILE).then_some((path, lines))
        })
        .collect();
    offenders.sort_by_key(|&(_, lines)| std::cmp::Reverse(lines));

    assert!(
        offenders.is_empty(),
        "these files exceed {MAX_LINES_PER_FILE} lines and should be split:\n{}",
        offenders
            .iter()
            .map(|(p, n)| format!(
                "  {n:>5} lines  {}",
                p.strip_prefix(&root).unwrap_or(p).display()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
