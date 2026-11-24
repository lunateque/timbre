#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

//! What the port answers when the library tree below it cannot be read.

use std::sync::Arc;

use timbre_kernel::folders::Folders;
use timbre_kernel::library_tree::{Folder, LibraryRoot, LibraryTree};
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    Ctx, Failure, FailureCode, Fallible, Format, LogFilter, Logger, Page, PageRequest, Port,
    Registry, ThreadRuntime,
};

const PAGE: PageRequest<'static> = PageRequest {
    limit: 50,
    cursor: None,
};

#[derive(Debug)]
struct Refused(FailureCode);

impl Failure for Refused {
    fn code(&self) -> FailureCode {
        self.0
    }

    fn status(&self) -> u16 {
        503
    }

    fn public_message(&self) -> &'static str {
        "The library is unavailable."
    }
}

/// A tree that answers every read with one failure, as when the database is
/// locked or the asked-for folder is gone.
struct Broken(FailureCode);

impl Port for Broken {}

impl LibraryTree for Broken {
    fn roots(&self, _: &Ctx, _: PageRequest<'_>) -> Fallible<Page<LibraryRoot>> {
        Err(Box::new(Refused(self.0)))
    }

    fn folder(&self, _: &Ctx, _: &str, _: PageRequest<'_>) -> Fallible<Folder> {
        Err(Box::new(Refused(self.0)))
    }

    fn tree_version(&self, _: &Ctx) -> Fallible<u64> {
        Ok(7)
    }
}

fn browse(code: FailureCode) -> (Ctx, Arc<dyn Folders>) {
    let mut registry = Registry::new();
    registry
        .provide::<dyn LibraryTree>(Arc::new(Broken(code)))
        .unwrap();
    let kernel = registry
        .module("folders", timbre_folders::register)
        .unwrap()
        .finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(Shared::default()),
                std::time::SystemTime::now,
            ),
            Arc::new(ThreadRuntime::new()),
        );
    let ctx = Ctx::root(&kernel);
    let folders = ctx.ports().get::<dyn Folders>().unwrap();
    (ctx, folders)
}

#[test]
fn an_unreadable_tree_fails_the_index_instead_of_serving_an_empty_one() {
    let (ctx, folders) = browse(FailureCode::new("library.unavailable"));

    // An empty index is a valid answer clients cache for a whole tree
    // version, so a tree that could not be read must fail loudly.
    let failure = folders.indexes(&ctx, None, None, PAGE).unwrap_err();
    assert_eq!(failure.code().as_str(), "library.unavailable");
}

#[test]
fn an_absent_folder_answers_with_the_folders_code() {
    let (ctx, folders) = browse(FailureCode::new("library.not_found"));

    let failure = folders.directory(&ctx, "404", None, PAGE).unwrap_err();
    assert_eq!(failure.code().as_str(), "folders.not_found");
}
