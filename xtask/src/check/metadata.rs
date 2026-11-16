//! The slice of `cargo metadata` the checks need, as plain structs.
//!
//! A dependency is internal when metadata gives it a `path` inside the workspace root;
//! the field is `null` for registry crates. Every dependency kind (normal, build, dev)
//! counts: `kind` exists to tell them apart in the metadata, not to exclude any of them
//! from the rules.

use crate::Outcome;
use crate::workspace;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PackageKind {
    Kernel,
    Binary,
    Module(String),
    Edge(String),
    Xtask,
    Unknown,
}

#[derive(Debug)]
pub(crate) struct Dep {
    pub(crate) name: String,
    /// The workspace-relative directory of the dependency, when it is an internal path
    /// dependency of this workspace.
    pub(crate) internal_dir: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Package {
    pub(crate) name: String,
    /// Workspace-relative directory of `manifest_path`, with `/`.
    pub(crate) dir: String,
    pub(crate) kind: PackageKind,
    pub(crate) deps: Vec<Dep>,
}

impl Package {
    /// The id every log event and failure code of this package must be prefixed with.
    pub(crate) fn module_id(&self) -> Option<&str> {
        match &self.kind {
            PackageKind::Kernel => Some("kernel"),
            PackageKind::Binary => Some("timbre"),
            PackageKind::Module(id) | PackageKind::Edge(id) => Some(id),
            PackageKind::Xtask | PackageKind::Unknown => None,
        }
    }
}

pub(crate) fn parse(value: &serde_json::Value, root: &Path) -> Outcome<Vec<Package>> {
    let packages = value
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata has no `packages` array".to_owned())?;
    let mut parsed = Vec::new();
    for entry in packages {
        let name = string_field(entry, "name")?;
        let manifest = string_field(entry, "manifest_path")?;
        let dir = workspace::relative(root, &PathBuf::from(&manifest))
            .and_then(|path| path.strip_suffix("/Cargo.toml").map(str::to_owned))
            .ok_or_else(|| {
                format!("`{name}` has a manifest outside the workspace: `{manifest}`")
            })?;
        let kind = kind_of(&dir);
        let mut deps = Vec::new();
        let empty: Vec<serde_json::Value> = Vec::new();
        for dep in entry
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty)
        {
            let dep_name = string_field(dep, "name")?;
            let internal_dir = dep
                .get("path")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| workspace::relative(root, &PathBuf::from(path)));
            deps.push(Dep {
                name: dep_name,
                internal_dir,
            });
        }
        parsed.push(Package {
            name,
            dir,
            kind,
            deps,
        });
    }
    Ok(parsed)
}

fn string_field(value: &serde_json::Value, key: &str) -> Outcome<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("cargo metadata entry has no string field `{key}`"))
}

fn kind_of(dir: &str) -> PackageKind {
    if dir == "crates/kernel" {
        return PackageKind::Kernel;
    }
    if dir == "crates/timbre" {
        return PackageKind::Binary;
    }
    if dir == "xtask" {
        return PackageKind::Xtask;
    }
    if let Some(id) = one_segment(dir.strip_prefix("crates/modules/")) {
        return PackageKind::Module(id.to_owned());
    }
    if let Some(name) = one_segment(dir.strip_prefix("crates/edge/")) {
        return PackageKind::Edge(name.to_owned());
    }
    PackageKind::Unknown
}

/// `Some(segment)` only for a direct child: non-empty and without another `/`.
fn one_segment(rest: Option<&str>) -> Option<&str> {
    rest.filter(|segment| !segment.is_empty() && !segment.contains('/'))
}
