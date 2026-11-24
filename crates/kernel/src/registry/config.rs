use super::{Kernel, Registry};
use crate::{Ctx, Fallible};
use std::sync::Arc;

impl Registry {
    /// Register a startup check that reads this module's runtime settings.
    pub fn config_check(&mut self, check: fn(&Ctx) -> Fallible) {
        self.config_checks.push(check);
    }
}

impl Kernel {
    /// Check module settings after resources are installed and before dispatch.
    /// The host calls this once, before starting handlers or jobs.
    ///
    /// # Errors
    ///
    /// Returns the first failed check in registration order.
    pub fn check_config(self: &Arc<Self>) -> Fallible {
        let ctx = Ctx::root(self);
        for check in &self.config_checks {
            check(&ctx)?;
        }
        Ok(())
    }
}
