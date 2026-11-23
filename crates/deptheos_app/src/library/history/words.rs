//! The history's sentences: a render's name as both apps write and speak
//! it, what Recently Deleted says, and Delete Library's question.

use super::{DeletedItem, Deletion, Took};
use crate::words::{Words, say};

/// A render's name in both apps: "#4", "#4 ★", "#4 ★ · sharper". A stack's
/// only render is "#1" too.
pub fn render_name(words: &Words, number: u32, name: Option<&str>, starred: bool) -> String {
    let number = number.to_string();
    match (name, starred) {
        (None, false) => say!(words, "render-number", number = number),
        (None, true) => say!(words, "render-number-starred", number = number),
        (Some(name), false) => say!(words, "render-number-named", number = number, name = name),
        (Some(name), true) => {
            say!(words, "render-number-named-starred", number = number, name = name)
        }
    }
}

/// A render in a menu of its stack's renders: [`render_name`], then its
/// method and settings ("#4 ★ · sharper · Pyramid · S4").
pub fn render_with_summary(
    words: &Words,
    number: u32,
    name: Option<&str>,
    starred: bool,
    summary: &str,
) -> String {
    let render = render_name(words, number, name, starred);
    say!(words, "render-with-summary", render = render, summary = summary)
}

/// A render as a screen reader says it: "Render 4, sharper, Pyramid,
/// starred", since "#4" reads as "number 4".
pub fn render_spoken(
    words: &Words,
    number: u32,
    name: Option<&str>,
    method: Option<&str>,
    starred: bool,
) -> String {
    let number = number.to_string();
    let Some(method) = method else { return say!(words, "render-spoken", number = number) };
    match (name, starred) {
        (None, false) => say!(words, "render-spoken-method", number = number, method = method),
        (None, true) => {
            say!(words, "render-spoken-method-starred", number = number, method = method)
        }
        (Some(name), false) => {
            say!(words, "render-spoken-named-method", number = number, name = name, method = method)
        }
        (Some(name), true) => say!(
            words,
            "render-spoken-named-method-starred",
            number = number,
            name = name,
            method = method
        ),
    }
}

/// A Recently Deleted record's title: "Bee", "Bee #2", "3 renders of Bee".
/// `untitled` names a stack with no name.
pub fn deleted_title(words: &Words, item: &DeletedItem, untitled: &str) -> String {
    let stack = item.name.as_deref().unwrap_or(untitled);
    match &item.took {
        Took::Stack { .. } => say!(words, "deleted-stack", stack = stack),
        Took::Renders { numbers, failed: 0 } => match numbers.as_slice() {
            [number] => say!(words, "deleted-render", stack = stack, number = number.to_string()),
            _ => say!(words, "deleted-renders", stack = stack, renders = numbers.len()),
        },
        Took::Renders { numbers, failed } if numbers.is_empty() => {
            say!(words, "deleted-failed", stack = stack, failed = failed.to_owned())
        }
        Took::Renders { numbers, failed } => say!(
            words,
            "deleted-renders-and-failed",
            stack = stack,
            renders = numbers.len(),
            failed = failed.to_owned()
        ),
    }
}

/// The notice after a delete: "Bee #2 moved to Recently Deleted.".
/// `untitled` as for [`deleted_title`].
pub fn deleted_notice(words: &Words, deletion: &Deletion, untitled: &str) -> String {
    match deletion.0.as_slice() {
        [item] => {
            let what = deleted_title(words, item, untitled);
            say!(words, "deleted-notice-one", what = what)
        }
        items if items.iter().all(|i| matches!(i.took, Took::Stack { .. })) => {
            say!(words, "deleted-notice-stacks", stacks = items.len())
        }
        items => {
            let renders = items
                .iter()
                .map(|i| match &i.took {
                    Took::Stack { renders } => (*renders).max(1),
                    Took::Renders { numbers, failed } => numbers.len() + failed,
                })
                .sum::<usize>();
            say!(words, "deleted-notice-renders", renders = renders)
        }
    }
}

/// A row's days left at `now`: "29 days left"; "1 day left" also once due.
pub fn deleted_days_left(words: &Words, item: &DeletedItem, now: std::time::SystemTime) -> String {
    say!(words, "deleted-days-left", days = (item.days_left(now).max(1) as usize))
}

/// Recently Deleted's subtitle: "4 items · 1.2 GB", or that it is empty.
/// `size` is formatted.
pub fn deleted_summary(words: &Words, items: usize, size: &str) -> String {
    match items {
        0 => say!(words, "deleted-none"),
        n => say!(words, "deleted-summary", items = n, size = size),
    }
}

/// Empty Recently Deleted's question: title, text and button. `frees` is
/// formatted.
pub fn empty_deleted_question(
    words: &Words,
    items: usize,
    frees: &str,
) -> (String, String, String) {
    (
        say!(words, "empty-deleted-title"),
        say!(words, "empty-deleted-message", items = items, size = frees),
        say!(words, "empty-deleted-button"),
    )
}

/// What leads the reason when a restore stopped part way.
pub fn restore_failed(words: &Words) -> String {
    say!(words, "restore-failed")
}

/// What leads the reason when emptying Recently Deleted stopped part way.
pub fn empty_deleted_failed(words: &Words) -> String {
    say!(words, "empty-deleted-failed")
}

/// Delete Library's question (Settings, on both apps): its title.
pub fn delete_library_title(words: &Words) -> String {
    say!(words, "delete-library-title")
}

/// Delete Library's confirming button.
pub fn delete_library_button(words: &Words) -> String {
    say!(words, "delete-library-button")
}

/// What Delete Library takes and leaves; `folder` where the app names it
/// (the desktop), `frees` once measured ([`History::render_sizes`]).
pub fn delete_library_message(
    words: &Words,
    stacks: usize,
    renders: usize,
    folder: Option<&str>,
    frees: Option<&str>,
) -> String {
    let what = match folder {
        Some(folder) => say!(
            words,
            "delete-library-message-in",
            stacks = stacks,
            renders = renders,
            folder = folder
        ),
        None => say!(words, "delete-library-message", stacks = stacks, renders = renders),
    };
    let mut sentences = vec![what];
    if let Some(size) = frees {
        sentences.push(say!(words, "delete-frees", size = size));
    }
    crate::words::sentences(words, &sentences)
}

/// The notice once Delete Library is done.
pub fn library_deleted(words: &Words) -> String {
    say!(words, "library-deleted")
}

/// What leads the reason when Delete Library stopped part way.
pub fn library_delete_failed(words: &Words) -> String {
    say!(words, "library-delete-failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_is_named_by_its_number_with_the_star_right_after_it() {
        let w = &Words::english(crate::Platform::Desktop);
        assert_eq!(render_name(w, 1, None, false), "#1");
        assert_eq!(render_name(w, 4, None, true), "#4 ★");
        assert_eq!(render_name(w, 4, Some("sharper"), false), "#4 · sharper");
        assert_eq!(render_name(w, 4, Some("sharper"), true), "#4 ★ · sharper");
        assert_eq!(
            render_with_summary(w, 4, Some("sharper"), true, "Pyramid · S4"),
            "#4 ★ · sharper · Pyramid · S4"
        );
        assert_eq!(render_spoken(w, 3, None, None, false), "Render 3");
        assert_eq!(render_spoken(w, 4, None, Some("Pyramid"), true), "Render 4, Pyramid, starred");
        assert_eq!(
            render_spoken(w, 4, Some("sharper"), Some("Pyramid"), false),
            "Render 4, sharper, Pyramid"
        );
    }

    #[test]
    fn delete_library_says_what_goes() {
        let w = &Words::english(crate::Platform::Desktop);
        assert_eq!(
            delete_library_message(w, 2, 1, Some("/Users/me/Deptheos"), Some("4 MB")),
            "2 stacks and 1 render in \"/Users/me/Deptheos\" are deleted, with their retouching, \
             any failed stacks, the working version and everything in Recently Deleted. Your \
             frames and exported files are left alone. This can't be undone. This frees 4 MB."
        );
        assert!(delete_library_message(w, 2, 0, None, None).starts_with("2 stacks and no renders"));
    }
}
