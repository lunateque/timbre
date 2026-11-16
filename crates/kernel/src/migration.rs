//! Migrations: the SQL a module ships to create and evolve its tables.

use crate::names;

/// One migration step of a module, applied by the database edge at startup.
#[derive(Clone, Debug)]
pub struct Migration {
    module: &'static str,
    version: u32,
    sql: &'static str,
}

impl Migration {
    pub(crate) const fn new(module: &'static str, version: u32, sql: &'static str) -> Self {
        Migration {
            module,
            version,
            sql,
        }
    }

    /// The owning module id; its tables carry this prefix.
    #[must_use]
    pub fn module(&self) -> &'static str {
        self.module
    }

    /// The step number within the module; versions apply in ascending order.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The SQL statements of this step.
    #[must_use]
    pub fn sql(&self) -> &'static str {
        self.sql
    }
}

/// Whether `module` and `version` may be registered together against `existing`.
pub(crate) fn accepted(module: &str, version: u32, existing: &[Migration]) -> bool {
    names::is_valid_segment(module)
        && !existing
            .iter()
            .any(|entry| entry.module == module && entry.version == version)
}

#[cfg(test)]
mod tests {
    use crate::{LogFilter, Logger, Registry};

    fn registry() -> Registry {
        Registry::new()
    }

    #[test]
    fn migrations_come_back_grouped_by_module_in_version_order() {
        let mut registry = registry();
        // Registered out of order, and interleaved with another module.
        registry
            .migration("library", 2, "CREATE TABLE library_b(a)")
            .expect("lib 2");
        registry
            .migration("auth", 1, "CREATE TABLE auth_a(a)")
            .expect("auth 1");
        registry
            .migration("library", 1, "CREATE TABLE library_a(a)")
            .expect("lib 1");
        let kernel = registry.finish(
            Logger::stdout(LogFilter::default()),
            std::sync::Arc::new(crate::ThreadRuntime::new()),
        );
        let seen: Vec<(&str, u32)> = kernel
            .migrations()
            .iter()
            .map(|entry| (entry.module(), entry.version()))
            .collect();
        assert_eq!(seen, [("library", 1), ("library", 2), ("auth", 1)]);
    }

    #[test]
    fn a_duplicate_version_or_a_bad_module_id_is_taken() {
        let mut registry = registry();
        registry.migration("auth", 1, "SELECT 1").expect("first");
        let duplicate = registry
            .migration("auth", 1, "SELECT 2")
            .expect_err("the same module and version twice");
        assert_eq!(duplicate.code(), crate::failures::MIGRATION_TAKEN);
        // Another module may use the same version number.
        registry
            .migration("library", 1, "SELECT 3")
            .expect("other module");
        let bad = registry
            .migration("Auth", 1, "SELECT 4")
            .expect_err("a module id is one name segment");
        assert_eq!(bad.code(), crate::failures::MIGRATION_TAKEN);
    }
}
