//! `tiers`: external crates live at the edge or in xtask, each listed in `deps.allow`.
//!
//! `deps.allow` is the ledger of every third-party crate in the tree: one crate per
//! line, followed by the reason it is allowed. A crate nobody uses anymore must leave
//! the ledger, so the file cannot quietly rot.

use super::Violation;
use super::metadata::{Package, PackageKind};

/// One accepted ledger line.
pub(crate) struct Entry {
    pub(crate) crate_name: String,
}

/// Parse the ledger, reporting malformed lines as we go.
fn parse(allow_text: &str, violations: &mut Vec<Violation>) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (number, line) in allow_text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let reason = parts.next();
        if name.contains('/') || reason.is_none() {
            violations.push(Violation::new(
                "tiers",
                "deps.allow",
                Some(number + 1),
                format!("`{trimmed}` is not `<crate> <reason>` with a non-empty reason"),
            ));
            continue;
        }
        entries.push(Entry {
            crate_name: name.to_owned(),
        });
    }
    entries
}

pub(crate) fn check(packages: &[Package], allow_text: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let listed = parse(allow_text, &mut violations);
    let names: Vec<&str> = listed
        .iter()
        .map(|entry| entry.crate_name.as_str())
        .collect();

    for package in packages {
        let may_have_external = matches!(package.kind, PackageKind::Edge(_) | PackageKind::Xtask);
        for dep in &package.deps {
            if dep.internal_dir.is_some() {
                continue;
            }
            if !may_have_external {
                violations.push(Violation::new(
                    "tiers",
                    &package.dir,
                    None,
                    format!(
                        "`{}` depends on the external crate `{}` outside the edge or xtask",
                        package.name, dep.name
                    ),
                ));
            } else if !names.contains(&dep.name.as_str()) {
                violations.push(Violation::new(
                    "tiers",
                    &package.dir,
                    None,
                    format!(
                        "the external crate `{}` is not listed in deps.allow",
                        dep.name
                    ),
                ));
            }
        }
    }

    for name in &names {
        let used = packages.iter().any(|package| {
            matches!(package.kind, PackageKind::Edge(_) | PackageKind::Xtask)
                && package
                    .deps
                    .iter()
                    .any(|dep| dep.internal_dir.is_none() && dep.name == *name)
        });
        if !used {
            violations.push(Violation::new(
                "tiers",
                "deps.allow",
                None,
                format!("`{name}` is listed but no edge or xtask package uses it"),
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::metadata::{Dep, Package, PackageKind};

    fn package(dir: &str, kind: PackageKind, external: &[&str]) -> Package {
        Package {
            name: dir.to_owned(),
            dir: dir.to_owned(),
            kind,
            deps: external
                .iter()
                .map(|dep| Dep {
                    name: (*dep).to_owned(),
                    internal_dir: None,
                })
                .collect(),
        }
    }

    const ALLOW: &str = "# comment\nserde_json reads the cargo metadata output in xtask\n";

    #[test]
    fn an_external_dep_in_a_module_is_reported() {
        let packages = [package(
            "crates/modules/hello",
            PackageKind::Module("hello".to_owned()),
            &["serde_json"],
        )];
        let violations = check(&packages, ALLOW);
        assert!(
            violations.iter().any(|v| v.path == "crates/modules/hello"
                && v.message.contains("outside the edge or xtask"))
        );
    }

    #[test]
    fn an_unlisted_external_dep_in_an_edge_crate_is_reported() {
        let packages = [package(
            "crates/edge/audio",
            PackageKind::Edge("audio".to_owned()),
            &["cpal"],
        )];
        let violations = check(&packages, ALLOW);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("`cpal` is not listed"))
        );
    }

    #[test]
    fn an_unlisted_external_dep_in_xtask_is_reported() {
        let packages = [package("xtask", PackageKind::Xtask, &["walkdir"])];
        let violations = check(&packages, ALLOW);
        assert!(
            violations
                .iter()
                .any(|v| v.path == "xtask" && v.message.contains("`walkdir` is not listed"))
        );
    }

    #[test]
    fn a_listed_crate_nobody_uses_is_reported() {
        let packages = [package("xtask", PackageKind::Xtask, &[])];
        let violations = check(&packages, ALLOW);
        assert!(
            violations
                .iter()
                .any(|v| v.path == "deps.allow" && v.message.contains("serde_json"))
        );
    }

    #[test]
    fn a_line_without_a_reason_is_reported() {
        let violations = check(&[], "# header\nserde_json\n");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].line, Some(2));
    }

    #[test]
    fn listed_and_used_is_clean() {
        let packages = [package("xtask", PackageKind::Xtask, &["serde_json"])];
        assert!(check(&packages, ALLOW).is_empty());
    }
}
