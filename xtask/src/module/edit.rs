//! Pure text edits behind `cargo xtask module`: every function returns the new file
//! content or an error naming the file and the anchor line it could not find, so a
//! failed edit leaves every file untouched by construction.
//!
//! Adding a line goes into the sorted block that starts right after the exact anchor
//! line; insertion and removal are inverses because removal matches the exact line,
//! wherever insertion had sorted it.

use crate::Outcome;

pub(crate) const ROOT_MANIFEST: &str = "Cargo.toml";
pub(crate) const BINARY_MANIFEST: &str = "crates/timbre/Cargo.toml";

const KERNEL_ROOT_DEP: &str = "timbre-kernel = { path = \"crates/kernel\" }";
const KERNEL_BINARY_DEP: &str = "timbre-kernel.workspace = true";

pub(crate) fn root_dependency(id: &str) -> String {
    format!("timbre-{id} = {{ path = \"crates/modules/{id}\" }}")
}

pub(crate) fn binary_dependency(id: &str) -> String {
    format!("timbre-{id} = {{ workspace = true, optional = true }}")
}

pub(crate) fn feature(id: &str) -> String {
    format!("{id} = [\"dep:timbre-{id}\"]")
}

pub(crate) fn default_line(ids: &[String]) -> String {
    if ids.is_empty() {
        return "default = []".to_owned();
    }
    let quoted: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
    format!("default = [{}]", quoted.join(", "))
}

pub(crate) fn insert_root_dependency(text: &str, id: &str) -> Outcome<String> {
    insert_line(
        text,
        ROOT_MANIFEST,
        KERNEL_ROOT_DEP,
        &root_dependency(id),
        |line| line.starts_with("timbre-"),
    )
}

pub(crate) fn remove_root_dependency(text: &str, id: &str) -> Outcome<String> {
    remove_line(text, ROOT_MANIFEST, &root_dependency(id))
}

pub(crate) fn insert_binary_dependency(text: &str, id: &str) -> Outcome<String> {
    insert_line(
        text,
        BINARY_MANIFEST,
        KERNEL_BINARY_DEP,
        &binary_dependency(id),
        |line| line.starts_with("timbre-"),
    )
}

pub(crate) fn remove_binary_dependency(text: &str, id: &str) -> Outcome<String> {
    remove_line(text, BINARY_MANIFEST, &binary_dependency(id))
}

/// The `[features]` section after adding `id` to the sorted `ids`: first the `default`
/// line is rewritten to the new list, then `<id> = ["dep:..."]` joins the sorted
/// feature block directly below it.
pub(crate) fn insert_feature(text: &str, id: &str, ids: &[String]) -> Outcome<String> {
    let text = set_default(text, ids)?;
    let anchor = default_line(ids);
    insert_line(&text, BINARY_MANIFEST, &anchor, &feature(id), |line| {
        !line.is_empty() && line.contains("[\"dep:timbre-")
    })
}

pub(crate) fn remove_feature(text: &str, id: &str, ids: &[String]) -> Outcome<String> {
    let text = remove_line(text, BINARY_MANIFEST, &feature(id))?;
    set_default(&text, ids)
}

/// The root `[workspace]` `members` line, rewritten wholesale. An explicit list, not a
/// `crates/modules/*` glob, because cargo treats an unmatched glob as a literal path and
/// the whole workspace stops loading once the last module is removed. Entries that are
/// not modules (the kernel, the binary, the edge crates, xtask) keep their order; the
/// sorted `crates/modules/<id>` block is (re)inserted right after `crates/timbre`, where
/// the old list carried it.
pub(crate) fn set_members(text: &str, ids: &[String]) -> Outcome<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    let modules: Vec<String> = sorted
        .iter()
        .map(|id| format!("\"crates/modules/{id}\""))
        .collect();
    let line_start = text
        .lines()
        .find(|line| line.starts_with("members = "))
        .ok_or_else(|| format!("{ROOT_MANIFEST}: the line `members = [...]` is missing"))?;
    let mut parts: Vec<String> = Vec::new();
    for entry in line_start
        .trim_start_matches("members = ")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
    {
        let entry = entry.trim();
        if entry.is_empty() || entry.trim_matches('"').starts_with("crates/modules/") {
            continue;
        }
        parts.push(entry.to_owned());
        if entry.trim_matches('"') == "crates/timbre" {
            parts.extend(modules.iter().cloned());
        }
    }
    let line = format!("members = [{}]", parts.join(", "));
    replace_prefix(text, "members = ", &line, ROOT_MANIFEST, "members = [...]")
}

fn set_default(text: &str, ids: &[String]) -> Outcome<String> {
    let replacement = default_line(ids);
    replace_prefix(
        text,
        "default = ",
        &replacement,
        BINARY_MANIFEST,
        "default = [...]",
    )
}

fn replace_prefix(
    text: &str,
    prefix: &str,
    replacement: &str,
    file: &str,
    described: &str,
) -> Outcome<String> {
    let mut lines: Vec<&str> = text.lines().collect();
    let keep_newline = text.ends_with('\n');
    let at = lines
        .iter()
        .position(|line| line.starts_with(prefix))
        .ok_or_else(|| format!("{file}: the line `{described}` is missing"))?;
    if let Some(slot) = lines.get_mut(at) {
        *slot = replacement;
    }
    Ok(join(&lines, keep_newline))
}

fn insert_line(
    text: &str,
    file: &str,
    anchor: &str,
    line: &str,
    in_block: impl Fn(&str) -> bool,
) -> Outcome<String> {
    let mut lines: Vec<&str> = text.lines().collect();
    let keep_newline = text.ends_with('\n');
    let at = lines
        .iter()
        .position(|candidate| *candidate == anchor)
        .ok_or_else(|| format!("{file}: the line `{anchor}` is missing"))?;
    if lines.contains(&line) {
        return Err(format!("{file}: the line `{line}` already exists"));
    }
    let mut at = at + 1;
    while let Some(candidate) = lines.get(at) {
        if in_block(candidate) && *candidate < line {
            at += 1;
        } else {
            break;
        }
    }
    lines.insert(at, line);
    Ok(join(&lines, keep_newline))
}

fn remove_line(text: &str, file: &str, line: &str) -> Outcome<String> {
    let mut lines: Vec<&str> = text.lines().collect();
    let keep_newline = text.ends_with('\n');
    let at = lines
        .iter()
        .position(|candidate| *candidate == line)
        .ok_or_else(|| format!("{file}: the line `{line}` is missing"))?;
    lines.remove(at);
    Ok(join(&lines, keep_newline))
}

fn join(lines: &[&str], ends_with_newline: bool) -> String {
    let mut text = lines.join("\n");
    if ends_with_newline {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> String {
        "[workspace.dependencies]\nra-ap-rustc_lexer = \"=0.185.0\"\nserde_json = \"1.0.154\"\ntimbre-kernel = { path = \"crates/kernel\" }\ntimbre-hello = { path = \"crates/modules/hello\" }\n\n[workspace.lints.rust]\n".to_owned()
    }

    fn binary() -> String {
        "[dependencies]\ntimbre-kernel.workspace = true\ntimbre-hello = { workspace = true, optional = true }\n\n[features]\ndefault = [\"hello\"]\nhello = [\"dep:timbre-hello\"]\n".to_owned()
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn root_dependency_inserts_sorted_and_removes() {
        let with_demo = insert_root_dependency(&root(), "demo").expect("anchor exists");
        assert!(
            with_demo
                .contains("timbre-kernel = { path = \"crates/kernel\" }\ntimbre-demo = { path = \"crates/modules/demo\" }\ntimbre-hello = { path = \"crates/modules/hello\" }")
        );
        let with_both = insert_root_dependency(&with_demo, "stream").expect("anchor exists");
        assert!(
            with_both.contains("timbre-hello = { path = \"crates/modules/hello\" }\ntimbre-stream")
        );
        let back = remove_root_dependency(
            &remove_root_dependency(&with_both, "stream").expect("present"),
            "demo",
        )
        .expect("present");
        assert_eq!(back, root());
    }

    #[test]
    fn binary_edits_add_and_remove_symmetrically() {
        let with_demo = insert_binary_dependency(&binary(), "demo").expect("anchor exists");
        let with_demo =
            insert_feature(&with_demo, "demo", &ids(&["demo", "hello"])).expect("anchor exists");
        assert!(with_demo.contains("timbre-kernel.workspace = true\ntimbre-demo = { workspace = true, optional = true }\ntimbre-hello ="));
        assert!(
            with_demo
                .contains("default = [\"demo\", \"hello\"]\ndemo = [\"dep:timbre-demo\"]\nhello = [\"dep:timbre-hello\"]")
        );
        let back = remove_binary_dependency(
            &remove_feature(&with_demo, "demo", &ids(&["hello"])).expect("present"),
            "demo",
        )
        .expect("present");
        assert_eq!(back, binary());
    }

    #[test]
    fn removing_the_last_feature_empties_default() {
        let text = remove_feature(&binary(), "hello", &ids(&[])).expect("anchor exists");
        assert!(text.contains("default = []\n"));
        assert!(!text.contains("hello = [\"dep:timbre-hello\"]"));
    }

    #[test]
    fn a_missing_anchor_is_an_error_naming_the_line() {
        let error =
            insert_root_dependency("[workspace.dependencies]\n", "demo").expect_err("no kernel");
        assert!(error.contains("Cargo.toml"), "{error}");
        assert!(error.contains(KERNEL_ROOT_DEP), "{error}");
        let error = remove_feature(&binary(), "demo", &ids(&["hello"])).expect_err("no demo");
        assert!(error.contains("demo = [\"dep:timbre-demo\"]"), "{error}");
    }

    #[test]
    fn a_duplicate_line_is_an_error() {
        let error = insert_root_dependency(&root(), "hello").expect_err("already there");
        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn members_list_is_rewritten_sorted() {
        let text = "[workspace]\nresolver = \"3\"\nmembers = [\"crates/kernel\", \"crates/timbre\", \"xtask\"]\n";
        let with = set_members(text, &ids(&["b", "a"])).expect("anchor exists");
        assert_eq!(
            with,
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/kernel\", \"crates/timbre\", \"crates/modules/a\", \"crates/modules/b\", \"xtask\"]\n"
        );
        assert_eq!(set_members(&with, &ids(&[])).expect("anchor exists"), text);
        assert!(set_members("[workspace]\n", &ids(&[])).is_err());
    }

    #[test]
    fn entries_that_are_not_modules_survive_the_rewrite() {
        // Edge crates sit between the binary and xtask and must not be lost
        // when the module block is rebuilt.
        let text = "[workspace]\nmembers = [\"crates/kernel\", \"crates/timbre\", \"crates/modules/old\", \"crates/edge/sqlite\", \"xtask\"]\n";
        let with = set_members(text, &ids(&["new"])).expect("anchor exists");
        assert_eq!(
            with,
            "[workspace]\nmembers = [\"crates/kernel\", \"crates/timbre\", \"crates/modules/new\", \"crates/edge/sqlite\", \"xtask\"]\n"
        );
    }
}
