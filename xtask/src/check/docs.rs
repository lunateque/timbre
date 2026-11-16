//! `docs`: every crate directory carries an `AGENTS.md` with the four contract headings.
//!
//! The headings are matched exactly on their own line: prose that merely mentions
//! `## Ports` inside a sentence is not a contract section.

use super::{Violation, metadata::Package};

const HEADINGS: &[&str] = &["## Purpose", "## Ports", "## Signals", "## Failures"];

/// `agents` holds the tracked contents of every `AGENTS.md` path.
pub(crate) fn check(agents: &[(&str, &str)], packages: &[Package]) -> Vec<Violation> {
    let mut violations = Vec::new();
    if !agents.iter().any(|(path, _)| *path == "AGENTS.md") {
        violations.push(Violation::new(
            "docs",
            "AGENTS.md",
            None,
            "the root AGENTS.md is not tracked",
        ));
    }
    for package in packages {
        let path = format!("{}/AGENTS.md", package.dir);
        let Some((_, text)) = agents.iter().find(|(candidate, _)| *candidate == path) else {
            violations.push(Violation::new(
                "docs",
                &path,
                None,
                "every package directory has a tracked AGENTS.md",
            ));
            continue;
        };
        for heading in HEADINGS {
            // `contains` over whole lines: a heading embedded in a longer line does not count.
            if !text.lines().any(|line| line.trim_end() == *heading) {
                violations.push(Violation::new(
                    "docs",
                    &path,
                    None,
                    format!("the heading `{heading}` is missing"),
                ));
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::metadata::{Package, PackageKind};

    fn package(dir: &str) -> Package {
        Package {
            name: dir.to_owned(),
            dir: dir.to_owned(),
            kind: PackageKind::Unknown,
            deps: Vec::new(),
        }
    }

    fn full() -> String {
        [
            "# Crate",
            "## Purpose",
            "does work",
            "## Ports",
            "## Signals",
            "## Failures",
        ]
        .join("\n")
    }

    #[test]
    fn a_missing_file_is_reported() {
        let packages = [package("crates/kernel")];
        let violations = check(&[("AGENTS.md", "# root")], &packages);
        assert!(
            violations
                .iter()
                .any(|v| v.path == "crates/kernel/AGENTS.md")
        );
    }

    #[test]
    fn one_missing_heading_is_reported() {
        let text = full().replace("## Signals", "## Something");
        let packages = [package("crates/kernel")];
        let agents = [
            ("AGENTS.md", "# root"),
            ("crates/kernel/AGENTS.md", text.as_str()),
        ];
        let violations = check(&agents, &packages);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("## Signals"));
    }

    #[test]
    fn a_heading_inside_a_longer_line_does_not_count() {
        let text = full().replace("## Ports", "text before ## Ports after");
        let packages = [package("crates/kernel")];
        let agents = [
            ("AGENTS.md", "# root"),
            ("crates/kernel/AGENTS.md", text.as_str()),
        ];
        assert!(
            check(&agents, &packages)
                .iter()
                .any(|v| v.message.contains("## Ports"))
        );
        let good = full();
        let agents = [
            ("AGENTS.md", "# root"),
            ("crates/kernel/AGENTS.md", good.as_str()),
        ];
        assert!(check(&agents, &packages).is_empty());
    }
}
