//! The desktop's message file checked against its code
//! (`deptheos_app::words::check`), `english.snap` recording every message.
//!
//! After a deliberate wording change, rewrite the snapshot with
//! `DEPTHEOS_UPDATE_SNAPSHOT=1 cargo test -p deptheos_gpui --bin deptheos-gui words`
//! and read its diff.

use std::path::Path;

use deptheos_app::words::check::Catalog;

use super::DesktopWords;

const ENGLISH: &str = include_str!("../../i18n/en-US/deptheos_gpui.ftl");

fn check(test: impl FnOnce(&Catalog)) {
    let words = DesktopWords::english();
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    test(&Catalog {
        english: ENGLISH,
        code: crate_dir.join("src"),
        loader: words.loader(),
        snapshot: crate_dir.join("src/words/english.snap"),
    });
}

#[test]
fn every_message_is_used_and_every_id_used_exists() {
    check(|c| c.assert_every_message_is_used_and_every_id_used_exists(500));
}

#[test]
fn every_term_is_used() {
    check(|c| c.assert_every_term_is_used());
}

#[test]
fn every_argument_is_named_in_a_comment() {
    check(|c| c.assert_every_argument_is_named_in_a_comment());
}

#[test]
fn every_message_renders_whole_as_the_snapshot_records() {
    check(|c| c.assert_renders_as_the_snapshot_records());
}
