//! The desktop's words (ADR-161): the shared [`Words`] plus the desktop's
//! own `i18n/<language>/deptheos_gpui.ftl` in the same language. `main`
//! makes one [`DesktopWords`] before any window and keeps it as a GPUI
//! global.
//!
//! A label or sentence is one message, worded with [`say!`]
//! (`say!(cx, "id", name = value)`); `fl!` checks ids and argument names
//! against the English file at build time. Numbers, sizes and durations are
//! passed as finished text; a count choosing a plural as a number.

use std::sync::Arc;

use deptheos_app::{Platform, Words};
use gpui::{App, Global};
use i18n_embed::LanguageLoader;
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;
use rust_embed::RustEmbed;

/// The desktop's message files, embedded.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Messages;

/// The shared layer's words and the desktop's own messages, in one
/// language. Cheap to clone: clones share the loaded messages.
#[derive(Clone)]
pub(crate) struct DesktopWords {
    shared: Words,
    own: Arc<FluentLanguageLoader>,
}

impl Global for DesktopWords {}

impl DesktopWords {
    /// The desktop's messages in `shared`'s language, English behind any
    /// missing (or all, without that language).
    pub(crate) fn new(shared: Words) -> Self {
        let loader = fluent_language_loader!();
        let requested: Vec<LanguageIdentifier> = shared.language().parse().into_iter().collect();
        match i18n_embed::select(&loader, &Messages, &requested) {
            Ok(languages) if !languages.is_empty() => {
                tracing::debug!("desktop words: {languages:?} for {requested:?}");
            }
            Ok(_) => load_english(&loader),
            Err(e) => {
                tracing::warn!("desktop words: {requested:?} couldn't be loaded, English: {e}");
                load_english(&loader);
            }
        }
        // No invisible direction marks round the arguments, as the shared
        // layer's words have none.
        loader.set_use_isolating(false);
        Self { shared, own: Arc::new(loader) }
    }

    /// The words for the system's preferred languages.
    pub(crate) fn system() -> Self {
        Self::new(Words::system(Platform::Desktop))
    }

    /// English: for tests.
    #[cfg(test)]
    pub(crate) fn english() -> Self {
        Self::new(Words::english(Platform::Desktop))
    }

    /// The shared layer's words, for the functions of `deptheos_app` that
    /// make sentences.
    pub(crate) fn shared(&self) -> &Words {
        &self.shared
    }

    /// The desktop's loaded messages, for [`say!`].
    pub(crate) fn loader(&self) -> &FluentLanguageLoader {
        &self.own
    }
}

fn load_english(loader: &FluentLanguageLoader) {
    let english = loader.fallback_language().clone();
    loader
        .load_languages(&Messages, &[english])
        .expect("the desktop's English messages are embedded and parse");
}

/// Keep `words` for the app; [`of`] and [`say!`] read them.
pub(crate) fn install(words: DesktopWords, cx: &mut App) {
    cx.set_global(words);
}

/// The shared layer's words [`install`] kept.
pub(crate) fn of(cx: &App) -> &Words {
    cx.global::<DesktopWords>().shared()
}

/// Where [`say!`] finds the words: the app (and through it any context),
/// or a [`DesktopWords`] in hand.
pub(crate) trait Wording {
    fn wording(&self) -> &DesktopWords;
}

impl Wording for App {
    fn wording(&self) -> &DesktopWords {
        self.global::<DesktopWords>()
    }
}

impl Wording for DesktopWords {
    fn wording(&self) -> &DesktopWords {
        self
    }
}

/// A desktop message as a `String`: `say!(cx, "id", name = value)`, `cx`
/// an app, a context or a [`DesktopWords`].
macro_rules! say {
    ($from:expr, $id:literal $(, $($args:tt)*)?) => {{
        #[allow(unused_imports)]
        use $crate::words::Wording as _;
        i18n_embed_fl::fl!($from.wording().loader(), $id $(, $($args)*)?)
    }};
}
pub(crate) use say;

#[cfg(test)]
mod catalog;
