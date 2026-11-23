//! The shared layer's message file checked against its code
//! ([`super::check`]) and `english.snap`. To accept new wording, run
//! `DEPTHEOS_UPDATE_SNAPSHOT=1 cargo test -p deptheos_app --lib words` and
//! read the diff.

use std::path::Path;

use super::Words;
use super::check::Catalog;
use crate::words::explain::Platform;

const ENGLISH: &str = include_str!("../../i18n/en-US/deptheos_app.ftl");

fn check(test: impl FnOnce(&Catalog)) {
    let words = Words::english(Platform::Desktop);
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
    check(|c| c.assert_every_message_is_used_and_every_id_used_exists(200));
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
