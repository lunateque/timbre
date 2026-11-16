//! CLI commands modules contribute to the binary.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::failures::KernelFailure;
use crate::future::BoxFuture;
use crate::names;

/// The handler shape of a CLI command: `args` are the words after the command
/// name; the handler prints and fails as it sees fit.
pub type CommandHandler = fn(Ctx, Vec<String>) -> BoxFuture<Fallible>;

struct Entry {
    name: String,
    words: Vec<String>,
    help: &'static str,
    handler: CommandHandler,
}

/// The command table; owned by the kernel, filled by `Registry::command`.
#[derive(Default)]
pub(crate) struct CommandTable {
    entries: Vec<Entry>,
}

/// One registered command as `help` lists it.
#[derive(Clone, Copy, Debug)]
pub struct Command<'a> {
    /// The words that invoke the command, e.g. `user add`.
    pub name: &'a str,
    /// One line shown by `timbre help`.
    pub help: &'static str,
}

impl CommandTable {
    pub(crate) fn add(
        &mut self,
        name: &str,
        help: &'static str,
        handler: CommandHandler,
    ) -> Fallible {
        let words: Vec<&str> = name.split(' ').collect();
        if words.iter().any(|word| !names::is_valid_segment(word)) {
            return Err(Box::new(KernelFailure::CommandInvalid {
                name: name.to_owned(),
            }));
        }
        if self.entries.iter().any(|entry| entry.words == words) {
            return Err(Box::new(KernelFailure::CommandTaken {
                name: name.to_owned(),
            }));
        }
        self.entries.push(Entry {
            name: name.to_owned(),
            words: words.into_iter().map(str::to_owned).collect(),
            help,
            handler,
        });
        Ok(())
    }

    /// Every command in registration order, for `help`.
    pub(crate) fn list(&self) -> Vec<Command<'_>> {
        self.entries
            .iter()
            .map(|entry| Command {
                name: &entry.name,
                help: entry.help,
            })
            .collect()
    }

    /// The command whose name is the longest prefix of `args`, with the words
    /// after the name handed to the handler.
    pub(crate) fn find(&self, args: &[String]) -> Option<(CommandHandler, Vec<String>)> {
        let mut best: Option<(&Entry, usize)> = None;
        for entry in &self.entries {
            let depth = entry.words.len();
            // `zip` stops at the shorter side, so compare the count first.
            if depth > args.len() || !args.iter().zip(&entry.words).all(|(arg, word)| arg == word) {
                continue;
            }
            if best.is_none_or(|(_, best_depth)| depth > best_depth) {
                best = Some((entry, depth));
            }
        }
        let (entry, depth) = best?;
        Some((entry.handler, args.iter().skip(depth).cloned().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use crate::failures::{COMMAND_INVALID, COMMAND_TAKEN};

    // Handlers are plain function pointers, so each records its own name on
    // the test `Recorder` port: tests tell commands apart by what was
    // recorded, not by comparing function pointers.
    fn user(ctx: Ctx, _: Vec<String>) -> BoxFuture<Fallible> {
        Box::pin(async move {
            if let Some(recorder) = ctx.ports().get::<dyn Recorder>() {
                recorder.push("user");
            }
            Ok(())
        })
    }

    fn user_add(ctx: Ctx, _: Vec<String>) -> BoxFuture<Fallible> {
        Box::pin(async move {
            if let Some(recorder) = ctx.ports().get::<dyn Recorder>() {
                recorder.push("user add");
            }
            Ok(())
        })
    }

    use crate::port::tests::{Recorder, RecorderImpl};
    use std::sync::Arc;

    fn kernel_with_recorder() -> (Arc<crate::Kernel>, Arc<RecorderImpl>) {
        let recorder = Arc::new(RecorderImpl::default());
        let mut registry = Registry::new();
        registry
            .provide::<dyn Recorder>(Arc::clone(&recorder) as Arc<dyn Recorder>)
            .expect("recorder");
        (
            registry.finish(
                crate::Logger::stdout(crate::LogFilter::default()),
                Arc::new(crate::ThreadRuntime::new()),
            ),
            recorder,
        )
    }

    fn table() -> CommandTable {
        let mut table = CommandTable::default();
        table
            .add("user add", "add a user", user_add)
            .expect("`user add` registers");
        table
            .add("user", "manage users", user)
            .expect("`user` registers");
        table
    }

    #[test]
    fn the_longest_command_name_wins_and_keeps_the_rest_for_the_handler() {
        let table = table();
        let (handler, rest) = table
            .find(&["user".to_owned(), "add".to_owned(), "anna".to_owned()])
            .expect("`user add` matches");
        assert_eq!(rest, ["anna".to_owned()]);
        let (kernel, recorder) = kernel_with_recorder();
        let answered = crate::block_on(handler(Ctx::root(&kernel), rest));
        assert!(answered.is_ok(), "the handler answered");
        assert_eq!(
            RecorderImpl::entries(&recorder),
            ["user add"],
            "the longer name's handler ran"
        );
    }

    #[test]
    fn a_shorter_prefix_still_matches_its_own_command() {
        let table = table();
        let (handler, rest) = table
            .find(&["user".to_owned()])
            .expect("the bare `user` command matches");
        assert!(rest.is_empty(), "no words after the name");
        let (kernel, recorder) = kernel_with_recorder();
        crate::block_on(handler(Ctx::root(&kernel), rest)).expect("the handler answered");
        assert_eq!(
            RecorderImpl::entries(&recorder),
            ["user"],
            "the shorter name's handler ran"
        );
        let (_, rest) = table
            .find(&["user".to_owned(), "remove".to_owned(), "anna".to_owned()])
            .expect("`user` is the only prefix of `user remove`");
        assert_eq!(rest, ["remove".to_owned(), "anna".to_owned()]);
        assert!(table.find(&["db".to_owned()]).is_none(), "unknown command");
    }

    #[test]
    fn a_duplicate_name_is_taken_and_a_malformed_name_is_invalid() {
        let mut table = table();
        let duplicate = table
            .add("user", "again", user)
            .expect_err("the name is registered twice");
        assert_eq!(duplicate.code(), COMMAND_TAKEN);
        for bad in ["", " user", "user ", "User", "user  add", "user-add"] {
            let error = table
                .add(bad, "malformed", user)
                .expect_err("names are lowercase words split by single spaces");
            assert_eq!(error.code(), COMMAND_INVALID, "for {bad:?}");
        }
        table
            .add("db check", "check the database", user)
            .expect("a new name registers");
    }
}
