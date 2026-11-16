//! Where the workspace is and which files belong to it.
//!
//! Cargo reports Windows paths with backslashes and git always reports forward slashes, so
//! every path handed to a check is normalized to a workspace-relative path with `/`.

use crate::Outcome;
use std::path::Path;

pub(crate) fn root() -> Outcome<std::path::PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot read the current directory: {error}"))?;
    let text = crate::process::output(
        "cargo",
        &["locate-project", "--workspace", "--message-format", "plain"],
        &cwd,
    )?;
    let manifest = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| "`cargo locate-project --workspace` printed no path".to_owned())?;
    let dir = Path::new(manifest)
        .parent()
        .ok_or_else(|| format!("`{manifest}` is not inside a directory"))?;
    std::path::absolute(dir).map_err(|error| format!("cannot resolve the workspace root: {error}"))
}

/// The tracked files, workspace-relative with `/` separators.
pub(crate) fn tracked_files(root: &Path) -> Outcome<Vec<String>> {
    let text = crate::process::output("git", &["ls-files", "-z"], root)?;
    let mut files = Vec::new();
    for entry in text.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let normalized = entry.replace('\\', "/");
        if normalized.contains("/../") || normalized.starts_with("../") {
            return Err(format!(
                "`git ls-files` escaped the workspace: {normalized}"
            ));
        }
        files.push(normalized);
    }
    Ok(files)
}

pub(crate) fn read(root: &Path, relative: &str) -> Outcome<String> {
    let path = root.join(relative);
    std::fs::read_to_string(&path).map_err(|error| format!("cannot read {relative}: {error}"))
}

/// `path` expressed as a workspace-relative path with `/`, or `None` when it lies outside
/// `root`. Comparison of the prefix ignores ASCII case because Windows path lookup does.
pub(crate) fn relative(root: &Path, path: &Path) -> Option<String> {
    let root_text = root.display().to_string().replace('\\', "/");
    let text = path.display().to_string().replace('\\', "/");
    let head = text.get(..root_text.len())?;
    if !head.eq_ignore_ascii_case(&root_text) {
        return None;
    }
    match text.get(root_text.len()..) {
        Some("") | None => Some(String::new()),
        Some(rest) => rest.strip_prefix('/').map(str::to_owned),
    }
}
