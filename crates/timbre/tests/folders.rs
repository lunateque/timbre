#[path = "../src/modules.rs"]
mod modules;

use std::sync::Arc;
use timbre_kernel::folders::Folders;
use timbre_kernel::{Ctx, LogFilter, Logger, PageRequest, ThreadRuntime};

const ALL: PageRequest<'static> = PageRequest {
    limit: 100,
    cursor: None,
};

/// The folder reached by walking these names down from the first root.
#[expect(
    clippy::unwrap_used,
    reason = "a missing folder must fail the test loudly"
)]
fn folder_at(
    ctx: &Ctx,
    tree: &Arc<dyn timbre_kernel::library_tree::LibraryTree>,
    names: &[&str],
) -> timbre_kernel::library_tree::Folder {
    use timbre_kernel::library_tree::FolderEntry;
    let roots = tree.roots(ctx, ALL).unwrap();
    let root = roots.items.first().unwrap();
    let mut folder = tree.folder(ctx, &root.folder_id, ALL).unwrap();
    for name in names {
        let id = folder
            .children
            .items
            .iter()
            .find_map(|entry| match entry {
                FolderEntry::Folder {
                    id, name: child, ..
                } if child == name => Some(id.clone()),
                _ => None,
            })
            .unwrap();
        folder = tree.folder(ctx, &id, ALL).unwrap();
    }
    folder
}

#[expect(
    clippy::unwrap_used,
    reason = "asserts exact failures in the removability test"
)]
fn assert_no_tree(ctx: &Ctx, folders: &dyn Folders) {
    assert!(
        ctx.ports()
            .get::<dyn timbre_kernel::library_tree::LibraryTree>()
            .is_none()
    );
    assert_eq!(
        folders.roots(ctx, ALL).unwrap_err().code().as_str(),
        "folders.not_available"
    );
    assert_eq!(
        folders
            .indexes(ctx, None, None, ALL)
            .unwrap_err()
            .code()
            .as_str(),
        "folders.not_available"
    );
    assert_eq!(
        folders
            .folder(ctx, "42", None, ALL)
            .unwrap_err()
            .code()
            .as_str(),
        "folders.not_available"
    );
}

mod with_library {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use timbre_edge_config::Config;
    use timbre_edge_sqlite::SharedDatabase;
    use timbre_kernel::folders::Versioned;
    use timbre_kernel::library_tree::{FolderEntry, LibraryFile, LibraryTree};
    use timbre_kernel::{Resources, block_on};

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one scanner lifecycle ties the directory id and version assertions together"
    )]
    #[test]
    fn filters_markers_and_nonmedia_and_carries_renamed_folder_ids() {
        let temp =
            Scratch(std::env::temp_dir().join(format!("timbre-folders-{}", std::process::id())));
        let root = temp.0.join("music");
        // The data directory must not contain a root, nor sit inside one.
        let data = temp.0.join("data");
        fs::create_dir_all(&data).unwrap();
        let audio = include_bytes!("../../edge/tags/tests/fixtures/tone.flac");
        for (name, bytes) in [
            ("Alpha/track.flac", audio.as_slice()),
            (
                "Alpha/cover.png",
                include_bytes!("../../edge/tags/tests/fixtures/cover.png").as_slice(),
            ),
            ("Alpha/track.lrc", b"[00:01] line".as_slice()),
            ("Alpha/notes.pdf", b"not visible"),
            ("Beta/Deep/track.flac", audio),
            ("Beta/.nomedia", b""),
            ("Gamma/Empty/song.flac", audio),
            ("Gamma/Empty/.timbreignore", b""),
            ("Plain/readme.txt.unknown", b"not visible"),
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let cfg = temp.0.join("timbre.toml");
        fs::write(
            &cfg,
            format!(
                "[library]\nroots = ['{}']\nscan_on_start = false\nwatch = false\n",
                root.display()
            ),
        )
        .unwrap();
        let resources = Resources::new()
            .with(Arc::new(Config::load(Some(&cfg)).unwrap()))
            .unwrap()
            .with(Arc::new(SharedDatabase::new(
                data.join("test.db"),
                |_, _| Ok(()),
            )))
            .unwrap();
        let kernel = modules::registry().unwrap().finish_with_resources(
            Logger::stdout(LogFilter::default()),
            Arc::new(ThreadRuntime::new()),
            resources,
        );
        kernel.check_config().unwrap();
        let ctx = Ctx::root(&kernel);
        let Some(folders) = ctx.ports().get::<dyn Folders>() else {
            return;
        };
        let Some((handler, _)) = kernel.find_command(&["scan".into()]) else {
            assert_no_tree(&ctx, folders.as_ref());
            return;
        };
        let scan = || {
            block_on(handler(ctx.clone(), Vec::new())).unwrap();
        };
        scan();
        let roots = folders.roots(&ctx, ALL).unwrap();
        assert_eq!(roots.items.len(), 1);
        let root_id = &roots.items[0].id;
        let Versioned::Changed {
            value: indexes,
            version,
        } = folders.indexes(&ctx, Some(root_id), None, ALL).unwrap()
        else {
            panic!("expected initial indexes")
        };
        assert_eq!(indexes.items.len(), 1);
        assert_eq!(indexes.items[0].letter, 'A');
        let initial = &indexes.items[0].folders[0];
        assert_eq!(initial.name, "Alpha");
        assert_eq!(
            folders
                .indexes(&ctx, Some(root_id), Some(version), ALL)
                .unwrap(),
            Versioned::NotModified
        );
        let first = PageRequest {
            limit: 1,
            cursor: None,
        };
        let Versioned::Changed {
            value: dir,
            version: dir_version,
        } = folders.folder(&ctx, &initial.id, None, first).unwrap()
        else {
            panic!("expected initial directory")
        };
        assert_eq!(dir_version, version);
        assert!(
            matches!(&dir.children.items[..], [FolderEntry::File { name, kind: LibraryFile::Image }] if name == "cover.png")
        );
        let next = PageRequest {
            limit: 100,
            cursor: dir.children.next.as_ref(),
        };
        let Versioned::Changed { value: dir, .. } =
            folders.folder(&ctx, &initial.id, None, next).unwrap()
        else {
            panic!("expected continuation")
        };
        assert!(
            matches!(&dir.children.items[..], [FolderEntry::File { name: audio, kind: LibraryFile::Audio { .. } }, FolderEntry::File { name: lyrics, kind: LibraryFile::Lyrics }] if audio == "track.flac" && lyrics == "track.lrc")
        );
        assert_eq!(
            folders
                .folder(&ctx, &initial.id, Some(version), ALL)
                .unwrap(),
            Versioned::NotModified
        );
        let tree = ctx.ports().get::<dyn LibraryTree>().unwrap();
        let ignored = folder_at(&ctx, &tree, &["Beta", "Deep"]);
        assert_eq!(
            folders
                .folder(&ctx, &ignored.id, None, ALL)
                .unwrap_err()
                .code()
                .as_str(),
            "folders.not_found"
        );
        fs::rename(root.join("Alpha"), root.join("Zeta")).unwrap();
        scan();
        let Versioned::Changed {
            value: renamed,
            version: changed,
        } = folders
            .folder(&ctx, &initial.id, Some(version), ALL)
            .unwrap()
        else {
            panic!("rename must change the version")
        };
        assert!(changed > version);
        assert_eq!(renamed.name, "Zeta");
        assert_eq!(renamed.id, initial.id);
    }
}
