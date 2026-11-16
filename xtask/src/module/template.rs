//! The files a new module crate is born with, as pure text so `module` writes them
//! without touching the disk in its tests.

/// The module crate manifest: identical to `hello`'s, with the package name swapped.
pub(crate) fn cargo_toml(id: &str) -> String {
    format!(
        "[package]\n\
         name = \"timbre-{id}\"\n\
         version.workspace = true\n\
         edition.workspace = true\n\
         rust-version.workspace = true\n\
         license.workspace = true\n\
         publish.workspace = true\n\
         \n\
         [lints]\n\
         workspace = true\n\
         \n\
         [dependencies]\n\
         timbre-kernel.workspace = true\n"
    )
}

/// The contract skeleton; `Purpose` says it is unwritten so no check mistakes it for prose.
pub(crate) fn agents(id: &str) -> String {
    format!(
        "# crates/modules/{id}\n\
         \n\
         ## Purpose\n\
         \n\
         A new module: its purpose must be written here before the first change lands.\n\
         \n\
         ## Ports\n\
         \n\
         None.\n\
         \n\
         ## Signals\n\
         \n\
         None.\n\
         \n\
         ## Failures\n\
         \n\
         None.\n"
    )
}

pub(crate) fn lib(id: &str) -> String {
    format!(
        "//! The {id} module.\n\
         \n\
         pub mod failures;\n\
         pub mod log_events;\n\
         \n\
         use timbre_kernel::{{Fallible, Registry}};\n\
         \n\
         /// Register the module.\n\
         ///\n\
         /// # Errors\n\
         ///\n\
         /// Never fails today; the signature is the module contract.\n\
         pub fn register(_registry: &mut Registry) -> Fallible {{\n\
        \x20   Ok(())\n\
         }}\n"
    )
}

pub(crate) fn log_events(id: &str) -> String {
    format!("//! The {id} module's log events.\n")
}

pub(crate) fn failures(id: &str) -> String {
    format!("//! The {id} module's failures.\n")
}
