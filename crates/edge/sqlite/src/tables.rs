//! Which module a stored table belongs to, and which names may be dropped.

/// The module that owns `name`: the registered module whose `<module>_`
/// prefix is the longest one `name` starts with. `stream_v2_rows` belongs to
/// `stream_v2`, not to `stream`, when both are registered.
///
/// The xtask `migrations` check enforces at build time that a module only
/// ever creates tables prefixed with its own name; this is the runtime side
/// of that rule, and anything that matches no registered module is orphaned.
pub(crate) fn owner_of<'a>(name: &str, registered_modules: &[&'a str]) -> Option<&'a str> {
    registered_modules
        .iter()
        .copied()
        .filter(|module| {
            name.len() > module.len()
                && name.starts_with(module)
                && name.as_bytes().get(module.len()) == Some(&b'_')
        })
        .max_by_key(|module| module.len())
}

/// Whether `name` is plain enough to interpolate into `DROP TABLE`.
pub(crate) fn valid_table_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_');
    first_ok && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
