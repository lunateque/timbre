//! A per-scan warning budget. One unreadable directory can hold thousands
//! of entries, and a line each would bury every other event in the log, so
//! only the first few are written in full and the rest are counted.

use crate::log_events::UNREADABLE_SUMMARY;
use timbre_kernel::{Ctx, log};

/// How many entries of one kind a single scan reports in full.
const WARN_LIMIT: u64 = 20;

#[derive(Default)]
pub(super) struct Warnings {
    logged: u64,
    total: u64,
}

impl Warnings {
    /// Count one failed entry and say whether it still fits the budget.
    pub(super) fn allow(&mut self) -> bool {
        self.total = self.total.saturating_add(1);
        if self.logged >= WARN_LIMIT {
            return false;
        }
        self.logged = self.logged.saturating_add(1);
        true
    }

    pub(super) fn total(&self) -> u64 {
        self.total
    }

    /// Report what the budget swallowed; `kind` names what was unreadable.
    pub(super) fn summarize(&self, ctx: &Ctx, kind: &'static str) {
        if self.total > self.logged {
            log!(
                ctx,
                warn,
                UNREADABLE_SUMMARY,
                kind = kind,
                unreadable = self.total,
                logged = self.logged
            );
        }
    }
}
