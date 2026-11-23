//! The language the user's sentences are worded in (ADR-161).
//!
//! A [`Words`] holds one language's embedded messages
//! (`i18n/<language>/deptheos_app.ftl`), English behind any it lacks. Each
//! front end makes one at start and passes it down; nothing is global, so
//! the functions that take one stay pure.
//!
//! A sentence is one message worded through `fl!`, which checks its id and
//! arguments against the English file at build time. Numbers and units
//! are passed as finished text; a count that picks a plural as a number.

pub mod explain;
pub mod ios;
pub mod units;

use std::sync::Arc;

use i18n_embed::LanguageLoader;
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;
use rust_embed::RustEmbed;

use crate::words::explain::Platform;

/// The message files, embedded.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Messages;

/// One loaded language, and the front end the words are for. Cheap to
/// clone: clones share the loaded messages.
#[derive(Clone)]
pub struct Words {
    loader: Arc<FluentLanguageLoader>,
    platform: Platform,
}

impl Words {
    /// The first of `requested` (tags such as "pt-BR", most wanted first)
    /// that has messages, else English; bad tags are skipped.
    pub fn new<S: AsRef<str>>(requested: &[S], platform: Platform) -> Self {
        let requested: Vec<LanguageIdentifier> =
            requested.iter().filter_map(|tag| tag.as_ref().parse().ok()).collect();
        let loader = fluent_language_loader!();
        let chosen = i18n_embed::select(&loader, &Messages, &requested);
        match chosen {
            Ok(languages) if !languages.is_empty() => {
                tracing::debug!("words: {languages:?} for {requested:?}");
            }
            Ok(_) => Self::load_english(&loader),
            Err(e) => {
                tracing::warn!("words: {requested:?} couldn't be loaded, English instead: {e}");
                Self::load_english(&loader);
            }
        }
        Self::finish(loader, platform)
    }

    /// The words for the system's preferred languages
    /// (`sys_locale::get_locales`), for the desktop and the command line.
    pub fn system(platform: Platform) -> Self {
        let requested: Vec<String> = sys_locale::get_locales().collect();
        Self::new(&requested, platform)
    }

    /// English, for tests and wherever no language was asked for.
    pub fn english(platform: Platform) -> Self {
        let loader = fluent_language_loader!();
        Self::load_english(&loader);
        Self::finish(loader, platform)
    }

    fn load_english(loader: &FluentLanguageLoader) {
        let english = loader.fallback_language().clone();
        loader
            .load_languages(&Messages, &[english])
            .expect("the English messages are embedded and parse");
    }

    fn finish(loader: FluentLanguageLoader, platform: Platform) -> Self {
        // No direction marks: sentences also go to logs and terminals.
        loader.set_use_isolating(false);
        Self { loader: Arc::new(loader), platform }
    }

    /// Which front end the words are for, where advice differs.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// The language the messages come from first, as a tag ("en-US").
    pub fn language(&self) -> String {
        self.loader.current_language().to_string()
    }

    /// The loaded messages, for `fl!`.
    pub(crate) fn loader(&self) -> &FluentLanguageLoader {
        &self.loader
    }
}

impl std::fmt::Debug for Words {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Words")
            .field("language", &self.language())
            .field("platform", &self.platform)
            .finish()
    }
}

/// The separator in compact metadata: "#4 · Pyramid · 46 frames"
/// (docs/terms-and-usage.md).
pub const SEPARATOR: &str = " · ";

/// Whole sentences one after another (`sentences`), skipping empty parts.
pub fn sentences<S: AsRef<str>>(words: &Words, parts: impl IntoIterator<Item = S>) -> String {
    fold(parts, |first, then| say!(words, "sentences", first = first, then = then))
}

/// Notice lines without full stops as sentences (`notices`), skipping
/// empty parts.
pub fn notices<S: AsRef<str>>(words: &Words, parts: impl IntoIterator<Item = S>) -> String {
    fold(parts, |first, then| say!(words, "notices", first = first, then = then))
}

/// The start of a list, before its last item (`list-comma`).
pub fn list<S: AsRef<str>>(words: &Words, items: impl IntoIterator<Item = S>) -> String {
    fold(items, |items, next| say!(words, "list-comma", items = items, next = next))
}

/// The non-empty `parts` joined pairwise by `join`, from the left.
fn fold<S: AsRef<str>>(
    parts: impl IntoIterator<Item = S>,
    join: impl Fn(String, &str) -> String,
) -> String {
    let mut text: Option<String> = None;
    for part in parts {
        let part = part.as_ref();
        if part.is_empty() {
            continue;
        }
        text = Some(match text {
            None => part.to_owned(),
            Some(text) => join(text, part),
        });
    }
    text.unwrap_or_default()
}

/// `say!(words, "id", name = value)`: `fl!` with a [`Words`]' loader.
macro_rules! say {
    ($words:expr, $id:literal $(, $($args:tt)*)?) => {
        i18n_embed_fl::fl!($words.loader(), $id $(, $($args)*)?)
    };
}
pub(crate) use say;

#[cfg(test)]
mod catalog;
#[cfg(any(test, feature = "test-support"))]
pub mod check;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_language_falls_back_to_english() {
        for requested in [&["xx-YY"][..], &[], &["not a tag"], &["en"], &["de-DE", "en-GB"]] {
            let words = Words::new(requested, Platform::Desktop);
            assert_eq!(words.language(), "en-US", "{requested:?}");
            assert_eq!(say!(words, "canceled"), "Stacking was canceled.");
        }
    }

    #[test]
    fn parts_are_joined_by_the_messages() {
        let words = Words::english(Platform::Desktop);
        assert_eq!(sentences(&words, ["One.", "", "Two. Three."]), "One. Two. Three.");
        assert_eq!(
            notices(&words, ["Left out a", "Clipped (2 frames)"]),
            "Left out a. Clipped (2 frames)"
        );
        assert_eq!(list(&words, ["\"a\"", "\"b\"", "\"c\""]), "\"a\", \"b\", \"c\"");
        assert_eq!(sentences(&words, Vec::<String>::new()), "");
    }

    #[test]
    fn arguments_carry_no_direction_marks() {
        let words = Words::english(Platform::Ios);
        let text = say!(words, "io-not-found", file = "\"a.jpg\"");
        assert_eq!(text, "\"a.jpg\" can't be found.");
    }
}
