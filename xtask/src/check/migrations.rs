//! `migrations`: every table a registered migration creates belongs to its module.
//!
//! A declaration is the token sequence `migration` `(` `<Str>` `,` `<Number>` `,` `<Str>`
//! outside `#[cfg(test)]` items. The module literal must equal the owning package's
//! module id, and every `CREATE TABLE|INDEX|VIRTUAL TABLE|TRIGGER|VIEW` name in the
//! SQL (case-insensitive, `IF NOT EXISTS` skipped, quotes stripped) must start with
//! `<module id>_`, the prefix `timbre db prune` later recognizes.

use super::metadata::Package;
use super::names::{owner_of, test_items, unquote};
use super::{Source, Violation};
use crate::lexer::{Kind, Token};

fn ident(token: &Token<'_>, name: &str) -> bool {
    token.kind == Kind::Ident && name.eq_ignore_ascii_case(token.text)
}

fn punct(token: &Token<'_>, text: &str) -> bool {
    token.kind == Kind::Punct && token.text == text
}

/// One `.migration(module, version, sql)` call: literal contents and the line
/// of the module literal.
struct Call<'a> {
    module: &'a str,
    sql: &'a str,
    line: usize,
}

/// The migration calls outside skipped token ranges.
fn calls<'a>(tokens: &[Token<'a>], skip: &[bool]) -> Vec<Call<'a>> {
    let mut found = Vec::new();
    for (start, window) in tokens.windows(7).enumerate() {
        if skip.get(start).is_some_and(|skipped| *skipped) {
            continue;
        }
        let [first, second, third, fourth, fifth, sixth, seventh] = window else {
            continue;
        };
        if !ident(first, "migration")
            || !punct(second, "(")
            || third.kind != Kind::Str
            || !punct(fourth, ",")
            || fifth.kind != Kind::Number
            || !punct(sixth, ",")
            || seventh.kind != Kind::Str
        {
            continue;
        }
        found.push(Call {
            module: unquote(third.text),
            sql: unquote(seventh.text),
            line: third.line,
        });
    }
    found
}

/// The SQL split into bare words: whitespace, punctuation and quotes separate.
fn words(sql: &str) -> Vec<&str> {
    sql.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '(' | ')' | ';' | ',' | '[' | ']' | '{' | '}' | '"' | '\'' | '`'
            )
    })
    .filter(|piece| !piece.is_empty())
    .collect()
}

/// The identifiers `CREATE [TEMP] [TEMPORARY] TABLE|INDEX|VIEW|TRIGGER
/// [IF NOT EXISTS] <name>` introduces, case-insensitively.
fn created_names(sql: &str) -> Vec<String> {
    let parts = words(sql);
    let upper: Vec<String> = parts
        .iter()
        .map(|piece| piece.to_ascii_uppercase())
        .collect();
    let mut names = Vec::new();
    let mut at = 0;
    while at < upper.len() {
        if upper.get(at).is_none_or(|word| word != "CREATE") {
            at += 1;
            continue;
        }
        let mut cursor = at + 1;
        // `TEMP`, `TEMPORARY`, `UNIQUE` and `OR REPLACE` precede the object keyword.
        while matches!(
            upper.get(cursor).map(String::as_str),
            Some("TEMP" | "TEMPORARY" | "UNIQUE" | "OR")
        ) {
            cursor += usize::from(upper.get(cursor).is_some_and(|word| word == "OR")) + 1;
        }
        let object = match upper.get(cursor).map(String::as_str) {
            Some("TABLE" | "INDEX" | "VIEW" | "TRIGGER") => 1,
            Some("VIRTUAL") if upper.get(cursor + 1).is_some_and(|w| w == "TABLE") => 2,
            _ => {
                at = cursor;
                continue;
            }
        };
        cursor += object;
        if upper.get(cursor).is_some_and(|word| word == "IF")
            && upper.get(cursor + 1).is_some_and(|word| word == "NOT")
            && upper.get(cursor + 2).is_some_and(|word| word == "EXISTS")
        {
            cursor += 3;
        }
        if let Some(name) = parts.get(cursor) {
            names.push((*name).to_owned());
        }
        at = cursor;
    }
    names
}

pub(crate) fn check(packages: &[Package], sources: &[Source<'_>]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in sources {
        let under_tests = file.path.split('/').any(|segment| segment == "tests");
        if under_tests {
            continue;
        }
        let skip = test_items(&file.tokens);
        for call in calls(&file.tokens, &skip) {
            let Some(owner) = owner_of(packages, file.path) else {
                continue;
            };
            let Some(module_id) = owner.module_id() else {
                violations.push(Violation::new(
                    "migrations",
                    file.path,
                    Some(call.line),
                    "a migration is registered here, but this package has no module id",
                ));
                continue;
            };
            if call.module != module_id {
                violations.push(Violation::new(
                    "migrations",
                    file.path,
                    Some(call.line),
                    format!(
                        "the migration names `{}`, but this package's module id is `{module_id}`",
                        call.module
                    ),
                ));
            }
            for name in created_names(call.sql) {
                let prefix = format!("{}_", call.module);
                if !name.starts_with(prefix.as_str()) {
                    violations.push(Violation::new(
                        "migrations",
                        file.path,
                        Some(call.line),
                        format!("created `{name}` must start with `{prefix}`"),
                    ));
                }
            }
        }
    }
    violations
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
            package(
                "crates/modules/hello",
                PackageKind::Module("hello".to_owned()),
            ),
        ]
    }

    fn call(sql: &str) -> String {
        format!("registry.migration(\"hello\", 1, \"{sql}\")?;\n")
    }

    #[test]
    fn a_correct_migration_is_clean() {
        let text = call("CREATE TABLE IF NOT EXISTS hello_greeting (id INTEGER)");
        let file = source("crates/modules/hello/src/lib.rs", &text);
        assert!(check(&packages(), &[file]).is_empty());
    }

    #[test]
    fn a_wrong_table_prefix_is_reported() {
        let text =
            call("CREATE TABLE greeting (id INTEGER); CREATE INDEX by_name ON greeting (id);");
        let file = source("crates/modules/hello/src/lib.rs", &text);
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(
            violations[0]
                .message
                .contains("`greeting` must start with `hello_")
        );
        assert!(violations[1].message.contains("`by_name`"));
    }

    #[test]
    fn a_wrong_module_literal_is_reported() {
        let file = source(
            "crates/modules/hello/src/lib.rs",
            "registry.migration(\"library\", 1, \"CREATE TABLE library_track (id INTEGER)\")?;\n",
        );
        let violations = check(&packages(), &[file]);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].message.contains("module id is `hello`"));
    }

    #[test]
    fn every_object_kind_and_quoting_is_checked() {
        let text = call(
            "CREATE VIRTUAL TABLE search USING fts5(a); \
                 CREATE TRIGGER bump AFTER INSERT ON x BEGIN SELECT 1; END; \
                 CREATE VIEW easy AS SELECT 1; create TEMP TABLE warm(a); \
             CREATE UNIQUE INDEX lookup ON t(x); \
             CREATE TABLE IF NOT EXISTS hello_ok(a);",
        );
        let file = source("crates/modules/hello/src/lib.rs", &text);
        let violations = check(&packages(), &[file]);
        let reported: Vec<&str> = violations.iter().map(|v| v.message.as_str()).collect();
        assert!(
            reported.iter().any(|m| m.contains("`search`")),
            "virtual table: {reported:?}"
        );
        assert!(
            reported.iter().any(|m| m.contains("`bump`")),
            "trigger: {reported:?}"
        );
        assert!(
            reported.iter().any(|m| m.contains("`easy`")),
            "view: {reported:?}"
        );
        assert!(
            reported.iter().any(|m| m.contains("`warm`")),
            "temp table: {reported:?}"
        );
        assert!(
            !reported.iter().any(|m| m.contains("hello_ok")),
            "the owned table stays clean: {reported:?}"
        );
        assert_eq!(violations.len(), 5, "{reported:?}");
    }

    #[test]
    fn a_quoted_name_has_its_quotes_stripped() {
        let text = call("CREATE TABLE \"hello_greeting\" (id INTEGER)");
        let file = source("crates/modules/hello/src/lib.rs", &text);
        assert!(check(&packages(), &[file]).is_empty());
    }

    #[test]
    fn a_migration_inside_cfg_test_is_ignored() {
        let text = "pub fn register(r: &mut Registry) -> Fallible {\n    r.migration(\"hello\", 1, \"CREATE TABLE hello_x(a)\")\n}\n#[cfg(test)]\nmod tests {\n    fn fixture() {\n        r.migration(\"wrong\", 1, \"CREATE TABLE nope(a)\");\n    }\n}\n";
        let file = source("crates/modules/hello/src/lib.rs", text);
        assert!(check(&packages(), &[file]).is_empty());
    }
}
