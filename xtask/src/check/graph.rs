//! `graph`: the dependency graph stays a star around the kernel, and module `cfg`
//! references live in the one generated file.
//!
//! Modules never depend on each other: they talk through kernel signals and ports.
//! `feature = "<id>"` outside `crates/timbre/src/modules.rs` would dangle the moment
//! the module is removed through `cargo xtask module remove`.

use super::metadata::{Package, PackageKind};
use super::{Source, Violation};

pub(crate) fn check(packages: &[Package], sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for package in packages {
        if package.kind == PackageKind::Unknown {
            violations.push(Violation::new(
                "graph",
                &package.dir,
                None,
                format!(
                    "`{}` is not a kernel, binary, module, edge or xtask directory",
                    package.name
                ),
            ));
            continue;
        }
        for dep in &package.deps {
            let Some(dir) = &dep.internal_dir else {
                continue;
            };
            let Some(target) = packages.iter().find(|candidate| candidate.dir == *dir) else {
                violations.push(Violation::new(
                    "graph",
                    &package.dir,
                    None,
                    format!(
                        "`{}` depends on `{dir}`, which is not a workspace package",
                        package.name
                    ),
                ));
                continue;
            };
            if !edge_allowed(&package.kind, &target.kind) {
                violations.push(Violation::new(
                    "graph",
                    &package.dir,
                    None,
                    format!(
                        "{} must not depend on {}",
                        describe(&package.kind),
                        describe(&target.kind)
                    ),
                ));
            }
        }
    }
    let module_ids: Vec<&str> = packages
        .iter()
        .filter_map(|package| {
            matches!(package.kind, PackageKind::Module(_))
                .then_some(package.module_id())
                .flatten()
        })
        .collect();
    for file in sources {
        if file.path == "crates/timbre/src/modules.rs" {
            continue;
        }
        // Token-based, not raw text: a comment mentioning `feature = "hello"` is not a
        // cfg reference, and `feature="hello"` without spaces is one.
        for window in file.tokens.windows(3) {
            let [name, equal, value] = window else {
                continue;
            };
            if name.kind == crate::lexer::Kind::Ident
                && name.text == "feature"
                && equal.kind == crate::lexer::Kind::Punct
                && equal.text == "="
                && value.kind == crate::lexer::Kind::Str
                && module_ids.iter().any(|id| *id == unquote(value.text))
            {
                violations.push(Violation::new(
                    "graph",
                    file.path,
                    None,
                    "module features are referenced only in crates/timbre/src/modules.rs",
                ));
            }
        }
    }
    violations
}

/// The inside of a plain string literal token; a `feature` value is never raw or escaped.
fn unquote(text: &str) -> &str {
    let inner = text.strip_prefix('"').unwrap_or(text);
    inner.strip_suffix('"').unwrap_or(inner)
}

fn edge_allowed(from: &PackageKind, to: &PackageKind) -> bool {
    match from {
        PackageKind::Kernel | PackageKind::Xtask | PackageKind::Unknown => false,
        PackageKind::Module(_) | PackageKind::Edge(_) => {
            matches!(to, PackageKind::Kernel | PackageKind::Edge(_))
        }
        PackageKind::Binary => matches!(
            to,
            PackageKind::Kernel | PackageKind::Module(_) | PackageKind::Edge(_)
        ),
    }
}

fn describe(kind: &PackageKind) -> String {
    match kind {
        PackageKind::Kernel => "the kernel".to_owned(),
        PackageKind::Binary => "the binary".to_owned(),
        PackageKind::Module(id) => format!("module `{id}`"),
        PackageKind::Edge(name) => format!("edge crate `{name}`"),
        PackageKind::Xtask => "xtask".to_owned(),
        PackageKind::Unknown => "an unknown package".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::metadata::Dep;
    use crate::check::source;

    fn package(dir: &str, kind: PackageKind, deps: &[&str]) -> Package {
        Package {
            name: dir.to_owned(),
            dir: dir.to_owned(),
            kind,
            deps: deps
                .iter()
                .map(|dep| Dep {
                    name: (*dep).to_owned(),
                    internal_dir: Some((*dep).to_owned()),
                })
                .collect(),
        }
    }

    fn workspace() -> Vec<Package> {
        vec![
            package("crates/kernel", PackageKind::Kernel, &[]),
            package(
                "crates/timbre",
                PackageKind::Binary,
                &["crates/kernel", "crates/modules/hello"],
            ),
            package(
                "crates/modules/hello",
                PackageKind::Module("hello".to_owned()),
                &["crates/kernel"],
            ),
        ]
    }

    #[test]
    fn a_star_workspace_is_clean() {
        assert!(check(&workspace(), &[]).is_empty());
    }

    #[test]
    fn a_module_depending_on_a_module_is_reported() {
        let mut packages = workspace();
        packages[2].deps.push(Dep {
            name: "timbre-a".to_owned(),
            internal_dir: Some("crates/modules/a".to_owned()),
        });
        packages.push(package(
            "crates/modules/a",
            PackageKind::Module("a".to_owned()),
            &[],
        ));
        let violations = check(&packages, &[]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "crates/modules/hello");
        assert!(violations[0].message.contains("module `a`"));
    }

    #[test]
    fn the_kernel_depending_on_a_module_is_reported() {
        let mut packages = workspace();
        packages[0].deps.push(Dep {
            name: "timbre-hello".to_owned(),
            internal_dir: Some("crates/modules/hello".to_owned()),
        });
        let violations = check(&packages, &[]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "crates/kernel");
    }

    #[test]
    fn a_package_outside_the_known_directories_is_unknown() {
        let packages = vec![package("tools/x", PackageKind::Unknown, &[])];
        let violations = check(&packages, &[]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "tools/x");
    }

    #[test]
    fn feature_text_outside_the_generated_file_is_reported() {
        let in_tests = source(
            "crates/timbre/tests/startup.rs",
            "if cfg!(feature = \"hello\") {}",
        );
        let in_generated = source(
            "crates/timbre/src/modules.rs",
            "#[cfg(feature = \"hello\")]",
        );
        assert_eq!(check(&workspace(), &[in_tests]).len(), 1);
        assert!(check(&workspace(), &[in_generated]).is_empty());
        let in_comment = source("a.rs", "// feature = \"hello\" is mentioned here\n");
        assert!(check(&workspace(), &[in_comment]).is_empty());
        let tight = source("a.rs", "if cfg!(feature=\"hello\") {}\n");
        assert_eq!(check(&workspace(), &[tight]).len(), 1);
    }
}
