//! `names`: every log event, failure code, permission and scheme name is prefixed
//! with its module id and is declared exactly once in the workspace.
//!
//! A declaration is the token sequence `LogEvent` `:` `:` `new` `(` `<Str>` (or the
//! same with `FailureCode`, `Permission` or `SchemeName`). Declarations in `tests/`
//! files and inside `#[cfg(test)]` items are ignored: test fixtures reuse names on
//! purpose and live where a reader expects them, and uniqueness of the shipped
//! names is what matters.

use super::metadata::Package;
use super::{Source, Violation};
use crate::lexer::{Kind, Token};

/// `Kind::Ident` with this exact text.
fn ident(token: &Token<'_>, name: &str) -> bool {
    token.kind == Kind::Ident && token.text == name
}

fn punct(token: &Token<'_>, text: &str) -> bool {
    token.kind == Kind::Punct && token.text == text
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Namespace {
    Event,
    Code,
    Permission,
    Scheme,
}

impl Namespace {
    fn label(self) -> &'static str {
        match self {
            Namespace::Event => "log event",
            Namespace::Code => "failure code",
            Namespace::Permission => "permission",
            Namespace::Scheme => "scheme name",
        }
    }

    /// The namespace a `<Type>::new("<literal>")` declaration belongs to, from
    /// the type's name.
    fn of_type(type_name: &str) -> Option<Self> {
        match type_name {
            "LogEvent" => Some(Self::Event),
            "FailureCode" => Some(Self::Code),
            "Permission" => Some(Self::Permission),
            "SchemeName" => Some(Self::Scheme),
            _ => None,
        }
    }
}

struct Declaration<'a> {
    namespace: Namespace,
    name: &'a str,
    line: usize,
}

pub(crate) fn check(packages: &[Package], sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    // events, codes, permissions and scheme names are separate namespaces, so a
    // duplicate is keyed by (name, namespace).
    let mut declared: Vec<(String, Namespace, String, usize)> = Vec::new();
    for file in sources {
        let skip = test_items(&file.tokens);
        for declaration in declarations(&file.tokens, &skip) {
            let under_tests = file.path.split('/').any(|segment| segment == "tests");
            if under_tests {
                continue;
            }
            let owner = owner_of(packages, file.path);
            let module_id = owner.and_then(Package::module_id);
            let prefix = match module_id {
                None => {
                    let who = owner.map_or_else(
                        || "no package".to_owned(),
                        |package| format!("`{}`", package.dir),
                    );
                    violations.push(Violation::new(
                        "names",
                        file.path,
                        Some(declaration.line),
                        format!("a {} is declared here, but {who} has no module id to prefix names with", declaration.namespace.label()),
                    ));
                    continue;
                }
                Some(id) => format!("{id}."),
            };
            if !declaration.name.starts_with(prefix.as_str()) {
                violations.push(Violation::new(
                    "names",
                    file.path,
                    Some(declaration.line),
                    format!("`{}` must start with `{prefix}`", declaration.name),
                ));
            }
            if let Some((_, _, first_path, first_line)) =
                declared.iter().find(|(name, namespace, _, _)| {
                    *name == declaration.name && *namespace == declaration.namespace
                })
            {
                violations.push(Violation::new(
                    "names",
                    file.path,
                    Some(declaration.line),
                    format!(
                        "{} `{}` is already declared in {first_path}:{first_line}",
                        declaration.namespace.label(),
                        declaration.name
                    ),
                ));
            } else {
                declared.push((
                    declaration.name.to_owned(),
                    declaration.namespace,
                    file.path.to_owned(),
                    declaration.line,
                ));
            }
        }
    }
    violations
}

/// The package whose `dir` is the longest workspace-relative prefix of `path`.
pub(crate) fn owner_of<'a>(packages: &'a [Package], path: &str) -> Option<&'a Package> {
    packages
        .iter()
        .filter(|package| path.starts_with(&format!("{}/", package.dir)))
        .max_by_key(|package| package.dir.len())
}

/// Token indexes covered by an item annotated `#[cfg(test)]`: the attribute is followed
/// by an item ending either at its depth-0 `;` or at its brace-balanced `{...}`.
pub(crate) fn test_items(tokens: &[Token<'_>]) -> Vec<bool> {
    let mut skip = vec![false; tokens.len()];
    let mut index = 0;
    while let Some(window) = tokens.get(index..index + 7) {
        let [
            hash,
            open,
            cfg,
            paren_open,
            test,
            paren_close,
            maybe_bracket,
        ] = window
        else {
            break;
        };
        let is_cfg_test = punct(hash, "#")
            && punct(open, "[")
            && ident(cfg, "cfg")
            && punct(paren_open, "(")
            && ident(test, "test")
            && punct(paren_close, ")");
        if !is_cfg_test || !punct(maybe_bracket, "]") {
            index += 1;
            continue;
        }
        let end = item_end(tokens, index + 7);
        for entry in skip.iter_mut().take(end + 1).skip(index) {
            *entry = true;
        }
        index = end + 1;
    }
    skip
}

/// Index of the last token of the item starting at `from`: its depth-0 `;`, or the
/// `}` closing the first depth-0 `{`.
fn item_end(tokens: &[Token<'_>], from: usize) -> usize {
    let mut depth = 0;
    let mut index = from;
    while let Some(token) = tokens.get(index) {
        if punct(token, "{") {
            depth += 1;
        } else if punct(token, "}") {
            depth -= 1;
            if depth == 0 {
                return index;
            }
        } else if depth == 0 && punct(token, ";") {
            return index;
        }
        index += 1;
    }
    tokens.len().saturating_sub(1)
}

/// The literal names declared outside skipped token ranges.
fn declarations<'a>(tokens: &[Token<'a>], skip: &[bool]) -> Vec<Declaration<'a>> {
    let mut found = Vec::new();
    for (start, window) in tokens.windows(6).enumerate() {
        if skip.get(start).is_some_and(|skipped| *skipped) {
            continue;
        }
        let [first, second, third, fourth, fifth, sixth] = window else {
            continue;
        };
        let namespace = match Namespace::of_type(first.text) {
            Some(namespace) if first.kind == Kind::Ident => namespace,
            _ => continue,
        };
        if !punct(second, ":")
            || !punct(third, ":")
            || !ident(fourth, "new")
            || !punct(fifth, "(")
            || sixth.kind != Kind::Str
        {
            continue;
        }
        let name = unquote(sixth.text);
        found.push(Declaration {
            namespace,
            name,
            line: sixth.line,
        });
    }
    found
}

/// The inside of a Rust string literal token; escapes are rare in names and the rules
/// here need only the prefix.
pub(crate) fn unquote(text: &str) -> &str {
    let inner = text.strip_prefix('"').unwrap_or(text);
    inner.strip_suffix('"').unwrap_or(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::metadata::{Package, PackageKind};
    use crate::check::source;

    fn package(dir: &str, kind: PackageKind) -> Package {
        Package {
            name: dir.to_owned(),
            dir: dir.to_owned(),
            kind,
            deps: Vec::new(),
        }
    }

    fn packages() -> Vec<Package> {
        vec![
            package("crates/kernel", PackageKind::Kernel),
            package("crates/timbre", PackageKind::Binary),
            package(
                "crates/modules/hello",
                PackageKind::Module("hello".to_owned()),
            ),
            package("xtask", PackageKind::Xtask),
        ]
    }

    #[test]
    fn a_wrong_prefix_is_reported() {
        let file = source(
            "crates/modules/hello/src/log_events.rs",
            "pub const E: LogEvent = LogEvent::new(\"timbre.greeted\");\n",
        );
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("must start with `hello.`"));
    }

    #[test]
    fn the_same_event_declared_twice_is_reported() {
        let first = source(
            "crates/modules/hello/src/log_events.rs",
            "pub const E: LogEvent = LogEvent::new(\"hello.greeted\");\n",
        );
        let second = source(
            "crates/modules/hello/src/other.rs",
            "pub const F: LogEvent = LogEvent::new(\"hello.greeted\");\n",
        );
        let violations = check(&packages(), &[first, second]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("already declared"));
    }

    #[test]
    fn a_declaration_in_xtask_is_reported() {
        let file = source(
            "xtask/src/check/names.rs",
            "const E: LogEvent = LogEvent::new(\"xtask.e\");\n",
        );
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("no module id"));
    }

    #[test]
    fn a_declaration_inside_cfg_test_is_ignored() {
        let file = source(
            "crates/kernel/src/log/logger.rs",
            "pub const E: LogEvent = LogEvent::new(\"kernel.e\");\n#[cfg(test)]\npub(crate) mod tests {\n    const G: LogEvent = LogEvent::new(\"hello.greeted\");\n}\n",
        );
        assert!(check(&packages(), &[file]).is_empty());
    }

    #[test]
    fn a_code_and_an_event_may_share_a_suffix() {
        let codes = source(
            "crates/kernel/src/failures.rs",
            "pub const C: FailureCode = FailureCode::new(\"kernel.log_filter_invalid\");\n",
        );
        let events = source(
            "crates/timbre/src/log_events.rs",
            "pub const E: LogEvent = LogEvent::new(\"timbre.log_filter_invalid\");\n",
        );
        assert!(check(&packages(), &[codes, events]).is_empty());
    }

    #[test]
    fn one_name_may_live_in_three_namespaces_of_one_package() {
        let file = source(
            "crates/modules/hello/src/lib.rs",
            "pub const C: FailureCode = FailureCode::new(\"hello.scan\");\n\
             pub const P: Permission = Permission::new(\"hello.scan\");\n\
             pub const S: SchemeName = SchemeName::new(\"hello.scan\");\n",
        );
        assert!(check(&packages(), &[file]).is_empty());
    }

    #[test]
    fn a_permission_with_a_foreign_prefix_is_reported() {
        let file = source(
            "crates/modules/hello/src/lib.rs",
            "pub const GREET: Permission = Permission::new(\"timbre.greet\");\n",
        );
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0]
                .message
                .contains("`timbre.greet` must start with `hello.`")
        );
    }

    #[test]
    fn a_scheme_with_a_foreign_prefix_is_reported() {
        let file = source(
            "crates/modules/hello/src/lib.rs",
            "pub const KEY: SchemeName = SchemeName::new(\"kernel.key\");\n",
        );
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].message.contains("must start with `hello.`"));
    }

    #[test]
    fn the_same_permission_declared_twice_is_reported() {
        let first = source(
            "crates/modules/hello/src/lib.rs",
            "pub const GREET: Permission = Permission::new(\"hello.greet\");\n",
        );
        let second = source(
            "crates/modules/hello/src/other.rs",
            "pub const HELLO_GREET: Permission = Permission::new(\"hello.greet\");\n",
        );
        let violations = check(&packages(), &[first, second]);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0]
                .message
                .contains("permission `hello.greet` is already declared")
        );
    }

    #[test]
    fn correct_permissions_and_schemes_are_clean() {
        let hello = source(
            "crates/modules/hello/src/lib.rs",
            "pub const GREET: Permission = Permission::new(\"hello.greet\");\n\
             pub const KEY: SchemeName = SchemeName::new(\"hello.key\");\n",
        );
        let kernel = source(
            "crates/kernel/src/credential/mod.rs",
            "pub const SIG: SchemeName = SchemeName::new(\"kernel.sig\");\n",
        );
        assert!(check(&packages(), &[hello, kernel]).is_empty());
    }
}
