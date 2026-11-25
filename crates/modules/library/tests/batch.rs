#[expect(
    dead_code,
    reason = "each test binary uses a subset of the shared fixture"
)]
mod support;

use support::{FIRST, Fixture};
use timbre_kernel::catalog::AlbumOrder;
use timbre_kernel::page::{MAX_PAGE_LIMIT, PageRequest};

#[test]
fn albums_by_ids_preserves_order_duplicates_and_omits_missing() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac(
        "one/a.flac",
        &[
            ("TITLE", "One"),
            ("ALBUM", "First"),
            ("ALBUMARTIST", "Alpha"),
        ],
    );
    fixture.flac(
        "two/b.flac",
        &[
            ("TITLE", "Two"),
            ("ALBUM", "Second"),
            ("ALBUMARTIST", "Beta"),
        ],
    );
    fixture.scan(false);
    let ctx = fixture.ctx();
    let catalog = Fixture::catalog(&ctx);
    let albums = catalog.albums(&ctx, AlbumOrder::Name, FIRST).unwrap().items;
    let first_album = albums.iter().find(|a| a.name == "First").unwrap();
    let second_album = albums.iter().find(|a| a.name == "Second").unwrap();

    let lookup = [
        second_album.id.as_str(),
        "999999",
        "not-an-id",
        first_album.id.as_str(),
        second_album.id.as_str(),
    ];
    let page1 = catalog
        .albums_by_ids(
            &ctx,
            &lookup,
            PageRequest {
                limit: 2,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(
        page1
            .items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Second", "First"]
    );
    let page2 = catalog
        .albums_by_ids(
            &ctx,
            &lookup,
            PageRequest {
                limit: 2,
                cursor: page1.next.as_ref(),
            },
        )
        .unwrap();
    assert_eq!(
        page2
            .items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Second"]
    );
    assert!(page2.next.is_none());
}

#[test]
fn artists_by_ids_and_page_limits_follow_the_catalog_contract() {
    let fixture = Fixture::new(&["music"]);
    fixture.flac("one/a.flac", &[("TITLE", "One"), ("ALBUMARTIST", "Alpha")]);
    fixture.flac("two/b.flac", &[("TITLE", "Two"), ("ALBUMARTIST", "Beta")]);
    fixture.scan(false);
    let ctx = fixture.ctx();
    let catalog = Fixture::catalog(&ctx);
    let artists = catalog.artists(&ctx, FIRST).unwrap().items;
    let alpha = artists.iter().find(|a| a.name == "Alpha").unwrap();
    let beta = artists.iter().find(|a| a.name == "Beta").unwrap();

    let batch = catalog
        .artists_by_ids(
            &ctx,
            &[
                beta.id.as_str(),
                "missing",
                alpha.id.as_str(),
                beta.id.as_str(),
            ],
            FIRST,
        )
        .unwrap();
    assert_eq!(
        batch
            .items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Beta", "Alpha", "Beta"]
    );

    let oversize = PageRequest {
        limit: MAX_PAGE_LIMIT + 1,
        cursor: None,
    };
    assert_eq!(
        catalog
            .albums_by_ids(&ctx, &[], oversize)
            .unwrap_err()
            .code(),
        timbre_kernel::failures::PAGE_TOO_LARGE
    );
    assert_eq!(
        catalog
            .artists_by_ids(&ctx, &[], oversize)
            .unwrap_err()
            .code(),
        timbre_kernel::failures::PAGE_TOO_LARGE
    );
    let too_many_ids = vec!["1"; MAX_PAGE_LIMIT + 1];
    assert_eq!(
        catalog
            .tracks(&ctx, &too_many_ids, FIRST)
            .unwrap_err()
            .code(),
        timbre_kernel::failures::PAGE_TOO_LARGE
    );
    assert_eq!(
        catalog
            .track_durations(&ctx, &too_many_ids)
            .unwrap_err()
            .code(),
        timbre_kernel::failures::PAGE_TOO_LARGE
    );

    // Non-canonical numeric spellings must not alias real ids.
    let real_track = fixture.id("One");
    let padded = format!("0{real_track}");
    let plus = format!("+{real_track}");
    assert_eq!(
        catalog
            .track_durations(&ctx, &[&padded, &plus, &real_track])
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        [real_track]
    );
}
