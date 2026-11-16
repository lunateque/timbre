//! `removability`: every module can leave the tree.
//!
//! The design rule is that any module is deleted with its directory; this command
//! proves the weaker daily form of it: with one module absent and all the others
//! present, the binary still compiles warning-free and its tests pass. `just ci`
//! already builds the all-modules case, so each combo here is everything-but-one.

use crate::Outcome;
use crate::workspace;
use crate::{process, report};

/// For each id, the sorted list of all the other ids; empty input yields no combos,
/// one id yields the single empty combo.
pub(crate) fn combos(ids: &[String]) -> Vec<Vec<String>> {
    ids.iter()
        .map(|kept| {
            ids.iter()
                .filter(|id| *id != kept)
                .cloned()
                .collect::<Vec<String>>()
        })
        .collect()
}

pub(crate) fn run() -> Outcome {
    let root = workspace::root()?;
    let files = workspace::tracked_files(&root)?;
    let ids = module_ids(&files);
    for combo in combos(&ids) {
        let label = if combo.is_empty() {
            "no modules".to_owned()
        } else {
            combo.join(", ")
        };
        report::line(&format!(
            "removability: building and testing timbre with {label}"
        ));
        let features = combo.join(",");
        let mut clippy = vec![
            "clippy",
            "-p",
            "timbre",
            "--no-default-features",
            "--all-targets",
            "--locked",
        ];
        let mut test = vec!["test", "-p", "timbre", "--no-default-features", "--locked"];
        if !combo.is_empty() {
            clippy.extend(["--features", features.as_str()]);
            test.extend(["--features", features.as_str()]);
        }
        clippy.extend(["--", "-D", "warnings"]);
        process::run("cargo", &clippy, &root)?;
        process::run("cargo", &test, &root)?;
    }
    Ok(())
}

/// The ids of tracked `crates/modules/<id>/Cargo.toml` files.
fn module_ids(files: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let rest = path.strip_prefix("crates/modules/")?;
            let id = rest.strip_suffix("/Cargo.toml")?;
            (!id.is_empty() && !id.contains('/')).then(|| id.to_owned())
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn zero_modules_have_no_combos() {
        assert_eq!(combos(&ids(&[])), Vec::<Vec<String>>::new());
    }

    #[test]
    fn one_module_yields_the_empty_combo() {
        assert_eq!(combos(&ids(&["hello"])), [ids(&[])]);
    }

    #[test]
    fn each_combo_is_everything_but_one_module() {
        assert_eq!(
            combos(&ids(&["a", "b", "c"])),
            [ids(&["b", "c"]), ids(&["a", "c"]), ids(&["a", "b"])]
        );
    }
}
