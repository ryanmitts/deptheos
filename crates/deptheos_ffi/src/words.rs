//! The bridge's language, and every sentence it words outside a call, each
//! a method of [`Words`], read against the `.ftl` in one place.

use std::sync::Arc;

use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::history::{self, Batch};
use deptheos_app::preferences::{self as shared, ExportPreferences, Preference};
use deptheos_app::{AlignMode, Platform, SettingHelp, TiffCompression, Words as AppWords};

use crate::{Algorithm, DeletedInfo};

/// A language for sentences worded outside an `Engine` (labels, titles,
/// plans, diagnostics errors): made at launch from
/// `Bundle.main.preferredLocalizations.first`, or the engine's own
/// (`Engine::words`). Cheap to share.
#[derive(uniffi::Object)]
pub struct Words {
    pub(crate) words: AppWords,
}

/// What a delete for good asks (Delete Library, Empty Recently Deleted):
/// title, what it takes and leaves, and its button.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DeleteLibraryQuestion {
    pub title: String,
    pub message: String,
    pub button: String,
}

#[uniffi::export]
impl Words {
    /// In `language` (such as "en" or "pt-BR"), or English when the engine
    /// lacks it.
    #[uniffi::constructor]
    pub fn new(language: String) -> Arc<Self> {
        Arc::new(Self { words: AppWords::new(&[language], Platform::Ios) })
    }

    // ---- a stack's settings

    /// What the settings call `method`, as on the desktop: "ECC (affine)".
    pub fn align_method_label(&self, method: AlignMode) -> String {
        deptheos_app::align_label(method, &self.words)
    }

    /// "Pyramid · S4": an algorithm's settings, as the desktop shows them.
    pub fn algorithm_summary(&self, algorithm: Algorithm) -> String {
        algorithm.config().summary(&self.words)
    }

    /// When to use `algorithm`'s method, as the desktop says under its
    /// choice.
    pub fn algorithm_description(&self, algorithm: Algorithm) -> String {
        algorithm.config().kind().description(&self.words)
    }

    /// What moving `setting` does, in one sentence, as on the desktop: an
    /// iOS section footer.
    pub fn setting_help(&self, setting: SettingHelp) -> String {
        setting.help(&self.words)
    }

    // ---- the preferences

    /// What the control is called: "JPEG quality", "Horizontal shift (%)".
    pub fn preference_label(&self, preference: Preference) -> String {
        preference.label(&self.words)
    }

    /// What it does, in a sentence or two: a section footer.
    pub fn preference_help(&self, preference: Preference) -> String {
        preference.help(&self.words)
    }

    /// "Grid search limits": the heading over the four limits.
    pub fn grid_limits_label(&self) -> String {
        shared::grid_limits_label(&self.words)
    }

    /// What the four limits do, under their heading.
    pub fn grid_limits_help(&self) -> String {
        shared::grid_limits_help(&self.words)
    }

    /// "ZIP", "LZW", "None": a compression as the choice offers it.
    pub fn tiff_compression_label(&self, compression: TiffCompression) -> String {
        shared::tiff_compression_label(compression, &self.words)
    }

    /// "For example: bee-4.jpg": what `export`'s pattern names a sample
    /// Depth render as `format` on `date` (`yyyy-MM-dd`).
    pub fn export_file_name_example(
        &self,
        export: ExportPreferences,
        format: ExportFormat,
        date: String,
    ) -> String {
        export.file_name_example(format.extension(), &date, &self.words)
    }

    // ---- the list and the result

    /// "Added 12 frames to canon1.": the notice over New Stack Instead.
    pub fn added_frames_text(&self, frames: u32, stack: String) -> String {
        deptheos_app::frames::list::add::added_text(frames as usize, &stack, &self.words)
    }

    /// "1 frame left out", "2 frames left out": the inspector's summary.
    pub fn left_out_line(&self, count: u32) -> String {
        deptheos_app::left_out_line(&self.words, count as usize)
    }

    /// "Stacked Nov 14, 2026 at 10:42" in Render Info; `date` is in the
    /// system's style.
    pub fn stacked_at_line(&self, date: String) -> String {
        deptheos_app::library::stacks::stacked_at(&date, &self.words)
    }

    // ---- the history

    /// A batch run's header: "Auto sort · 9 frames skipped".
    pub fn batch_label(&self, batch: Batch) -> String {
        batch.label.text(&self.words)
    }

    /// A render by its number, as both apps name it: "#4", the star after
    /// the number and its name after ("#4 ★ · sharper"),
    /// `history::render_name`.
    pub fn render_name(&self, number: u32, name: Option<String>, picked: bool) -> String {
        history::render_name(&self.words, number, name.as_deref(), picked)
    }

    /// In a menu of its stack's renders: "#4 ★ · sharper · Pyramid · S4"
    /// (`history::render_with_summary`).
    pub fn render_menu_title(
        &self,
        number: u32,
        name: Option<String>,
        picked: bool,
        algorithm: Algorithm,
    ) -> String {
        let summary = algorithm.config().summary(&self.words);
        history::render_with_summary(&self.words, number, name.as_deref(), picked, &summary)
    }

    /// As VoiceOver says it: "Render 4, sharper, Pyramid, starred"; with no
    /// `algorithm`, "Render 4" (`history::render_spoken`).
    pub fn render_spoken(
        &self,
        number: u32,
        name: Option<String>,
        algorithm: Option<Algorithm>,
        picked: bool,
    ) -> String {
        let method = algorithm.map(|a| a.config().kind().label(&self.words));
        history::render_spoken(&self.words, number, name.as_deref(), method.as_deref(), picked)
    }

    /// A Recently Deleted row's name: "Bee", "Bee #2", "3 renders of Bee"
    /// (`history::deleted_title`); `untitled` names a stack with no name.
    pub fn deleted_title(&self, item: DeletedInfo, untitled: String) -> String {
        match item.item() {
            Some(item) => history::deleted_title(&self.words, &item, &untitled),
            None => untitled,
        }
    }

    /// "Bee #2 moved to Recently Deleted.", beside Undo, of the records made
    /// (`history::deleted_notice`).
    pub fn deleted_notice(&self, items: Vec<DeletedInfo>, untitled: String) -> String {
        let deletion = history::Deletion(items.iter().filter_map(DeletedInfo::item).collect());
        history::deleted_notice(&self.words, &deletion, &untitled)
    }

    /// "29 days left" (`history::deleted_days_left`).
    pub fn deleted_days_left(&self, item: DeletedInfo) -> String {
        let now = std::time::SystemTime::now();
        item.item().map_or_else(String::new, |i| history::deleted_days_left(&self.words, &i, now))
    }

    /// "4 items · 1.2 GB", or that nothing was deleted
    /// (`history::deleted_summary`); `size` is formatted.
    pub fn deleted_summary(&self, items: u32, size: String) -> String {
        history::deleted_summary(&self.words, items as usize, &size)
    }

    /// Empty Recently Deleted's question, of `items` rows freeing `frees`,
    /// formatted (`history::empty_deleted_question`).
    pub fn empty_deleted_question(&self, items: u32, frees: String) -> DeleteLibraryQuestion {
        let (title, message, button) =
            history::empty_deleted_question(&self.words, items as usize, &frees);
        DeleteLibraryQuestion { title, message, button }
    }

    /// What leads the reason when a restore stopped part way.
    pub fn restore_failed(&self) -> String {
        history::restore_failed(&self.words)
    }

    /// What leads the reason when emptying Recently Deleted stopped part
    /// way.
    pub fn empty_deleted_failed(&self) -> String {
        history::empty_deleted_failed(&self.words)
    }

    /// What Delete Library asks, of `stacks` and `renders`
    /// (`history::delete_library_title`, `delete_library_message`,
    /// `delete_library_button`); `frees` is the renders' total
    /// (`Engine::render_sizes`), formatted, once all are measured.
    pub fn delete_library_question(
        &self,
        stacks: u32,
        renders: u32,
        frees: Option<String>,
    ) -> DeleteLibraryQuestion {
        let w = &self.words;
        DeleteLibraryQuestion {
            title: history::delete_library_title(w),
            message: history::delete_library_message(
                w,
                stacks as usize,
                renders as usize,
                None,
                frees.as_deref(),
            ),
            button: history::delete_library_button(w),
        }
    }

    /// What leads the reason when Delete Library stopped part way.
    pub fn library_delete_failed(&self) -> String {
        history::library_delete_failed(&self.words)
    }
}
