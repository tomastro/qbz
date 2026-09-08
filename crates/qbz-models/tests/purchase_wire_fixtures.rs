//! The purchase envelopes, checked against REAL CAPTURED BYTES.
//!
//! # Why these are not the synthetic tests in `types.rs`
//!
//! The unit tests beside the models assert the shapes we BELIEVE Qobuz sends.
//! These assert the shapes it ACTUALLY sent. The files in `tests/fixtures/purchases/`
//! were captured on 2026-08-15 against the owner's own account, read-only and with
//! their authorisation, and the ids and login were redacted afterwards. They
//! replace the inferred dummy response this project started from — before them,
//! every claim about the purchase envelope was a reading of somebody else's code.
//!
//! They matter more than usual here because nobody on this team can smoke-test
//! Purchases: Qobuz does not sell it in the owner's region, so no screen will
//! ever be exercised locally. A fixture is the only thing in the loop that
//! cannot drift from what the server really does.
//!
//! # What they can and cannot prove
//!
//! Every capture is from an account that owns NOTHING, so `items` is empty in
//! all of them. They pin the ENVELOPE — which keys exist, which are absent, what
//! the pagination scalars look like — and they cannot say anything about a
//! populated item. That gap is real and is why the contract's §11-1 question
//! could not be answered from this account.

use qbz_models::{PurchaseIdsResponse, PurchaseResponse};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/purchases")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing capture {}: {e}", path.display()))
}

/// The unfiltered call returns BOTH pages, and the server's default `limit` is
/// 50 — not 500, which is what the client asks for when it paginates.
#[test]
fn unfiltered_purchases_carry_both_pages_and_a_default_limit_of_50() {
    let r: PurchaseResponse = serde_json::from_str(&fixture("getUserPurchases-noparams.json"))
        .expect("the captured envelope must deserialize");

    assert_eq!(r.albums.limit, 50, "the server's default page size");
    assert_eq!(r.tracks.limit, 50);
    assert_eq!(r.albums.offset, 0);
    assert_eq!(r.albums.total, 0);
    assert!(r.albums.items.is_empty());
    assert!(r.tracks.items.is_empty());
}

/// THE correction that mattered: `?type=albums` **omits the `tracks` key
/// entirely**. It is not present-and-zeroed, which is what the first reading of
/// the reference concluded from a function that zeroes the sibling total.
///
/// The observable effect is the same — an empty sibling page — which is why the
/// two-`getUserPurchasesIds`-calls requirement stands either way. But a port that
/// models a present-but-empty sibling is modelling something the server does not
/// send, and it is exactly the assumption that made the reference's album
/// "downloaded" predicate unsatisfiable (contract §11-1).
#[test]
fn a_typed_albums_response_omits_the_tracks_key_entirely() {
    let raw = fixture("getUserPurchases-type-albums.json");
    assert!(
        !raw.contains("\"tracks\""),
        "the capture must NOT contain a tracks key — that absence is the point"
    );

    let r: PurchaseResponse = serde_json::from_str(&raw).expect("must still deserialize");
    assert_eq!(
        r.albums.limit, 500,
        "the client's requested page size comes back"
    );
    assert!(
        r.tracks.items.is_empty(),
        "the absent sibling defaults, never fails"
    );
    assert_eq!(r.tracks.total, 0);
}

/// And symmetrically for tracks, so nobody assumes the omission is albums-only.
#[test]
fn a_typed_tracks_response_omits_the_albums_key_entirely() {
    let raw = fixture("getUserPurchases-type-tracks.json");
    assert!(!raw.contains("\"albums\""));

    let r: PurchaseResponse = serde_json::from_str(&raw).expect("must still deserialize");
    assert_eq!(r.tracks.limit, 500);
    assert!(r.albums.items.is_empty());
}

/// The ids envelope is a DIFFERENT page shape: `{total, items}` with no `offset`
/// and no `limit`.
///
/// This is the one that would have failed loudly and been misdiagnosed. Both tab
/// counters are read from `.total` here, and the page sits behind a lenient
/// deserializer that swallows a parse failure into an EMPTY page. Had `offset`
/// and `limit` been required fields, both counters would have read 0 forever with
/// nothing logged, and the obvious conclusion would have been "the user owns
/// nothing" rather than "the page did not parse".
#[test]
fn the_ids_envelope_has_no_offset_or_limit_and_still_yields_its_total() {
    let raw = fixture("getUserPurchasesIds-noparams.json");
    assert!(
        !raw.contains("\"offset\""),
        "the ids page carries no offset"
    );
    assert!(!raw.contains("\"limit\""), "nor a limit");

    let r: PurchaseIdsResponse = serde_json::from_str(&raw).expect("must deserialize");
    assert_eq!(r.albums.total, 0);
    assert_eq!(r.tracks.total, 0);
    assert_eq!(r.albums.offset, 0, "defaulted, not sent");
    assert_eq!(r.albums.limit, 0, "defaulted, not sent");
}

/// The typed ids call drops the sibling too — so the per-type totals genuinely
/// need one call each, which is what the reference does and what the contract
/// requires. A two-parameter port that fires a single combined call reads one
/// type's total and silently reports 0 for the other.
#[test]
fn a_typed_ids_response_omits_its_sibling_which_is_why_two_calls_are_needed() {
    let raw = fixture("getUserPurchasesIds-type-albums.json");
    assert!(!raw.contains("\"tracks\""));

    let r: PurchaseIdsResponse = serde_json::from_str(&raw).expect("must deserialize");
    assert_eq!(r.albums.total, 0);
    assert_eq!(
        r.tracks.total, 0,
        "absent sibling defaults to 0, never fails"
    );
}

/// Every capture carries a top-level `user` block that no model declares, and
/// none of them may fail because of it. Unknown top-level keys must be ignored —
/// if Qobuz adds another one tomorrow, the purchases screen must not go blank.
#[test]
fn an_unknown_top_level_key_never_fails_a_response() {
    for name in [
        "getUserPurchases-noparams.json",
        "getUserPurchases-type-albums.json",
        "getUserPurchases-type-tracks.json",
    ] {
        let raw = fixture(name);
        assert!(
            raw.contains("\"user\""),
            "{name} should carry the user block"
        );
        serde_json::from_str::<PurchaseResponse>(&raw)
            .unwrap_or_else(|e| panic!("{name} must ignore the unknown `user` key: {e}"));
    }
    for name in [
        "getUserPurchasesIds-noparams.json",
        "getUserPurchasesIds-type-albums.json",
    ] {
        serde_json::from_str::<PurchaseIdsResponse>(&fixture(name))
            .unwrap_or_else(|e| panic!("{name} must ignore the unknown `user` key: {e}"));
    }
}

/// FIRST capture from an account that OWNS albums (2026-09-01, USA Sublime
/// account, read-only, owner-authorised). Every prior fixture had empty items,
/// so this is the first proof of the POPULATED shape. If the whole page
/// deserializes to zero items while the raw JSON plainly has three, the bug is
/// in a per-item field the strict struct rejects — not downstream.
#[test]
fn a_populated_albums_page_yields_its_three_items() {
    let raw = fixture("getUserPurchases-albums-populated.json");
    let r: PurchaseResponse =
        serde_json::from_str(&raw).expect("the populated envelope must deserialize");
    assert_eq!(
        r.albums.items.len(),
        3,
        "expected 3 purchased albums, got {} (total={})",
        r.albums.items.len(),
        r.albums.total
    );
}

/// The same capture is the first proof of the ENTITLEMENT fields. The DSD64
/// album streams as 16/44.1 (`hires:false`) yet is purchased as `[55]` with
/// `hires_purchased:true` — the streaming quality says nothing about what was
/// bought, so the ids must survive the parse verbatim, in wire order.
#[test]
fn a_populated_albums_page_carries_the_downloadable_format_ids() {
    let raw = fixture("getUserPurchases-albums-populated.json");
    let r: PurchaseResponse = serde_json::from_str(&raw).expect("deserializes");
    let by_title: std::collections::HashMap<&str, &qbz_models::PurchaseAlbum> = r
        .albums
        .items
        .iter()
        .map(|a| (a.title.as_str(), a))
        .collect();

    let dsd64 = by_title["Rust In Peace"];
    assert_eq!(dsd64.downloadable_format_ids, vec![55]);
    assert!(!dsd64.hires, "the streaming catalog is CD quality");
    assert!(dsd64.hires_purchased, "…but the purchase is hi-res");

    let flac = by_title["ぷりぷり"];
    assert_eq!(flac.downloadable_format_ids, vec![7, 6, 5]);

    let dsd128 = by_title["Audiophile Analog Collection Vol.3"];
    assert_eq!(dsd128.downloadable_format_ids, vec![56]);
}
