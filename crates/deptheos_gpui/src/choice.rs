//! Dropdowns over typed values. gpui-kit's `Select` keeps its committed
//! selection, so the value is read from it (`chosen`), and each item
//! carries its value, so no label maps back.

use gpui::{App, AppContext as _, Entity, SharedString, Window};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::SelectState;

use crate::prefs::Keyed;
use crate::words::DesktopWords;

/// One dropdown entry: what it says and what it means.
#[derive(Clone)]
pub struct Choice<T> {
    pub label: SharedString,
    pub value: T,
}

impl<T> Choice<T> {
    pub fn new(label: impl Into<SharedString>, value: T) -> Self {
        Self { label: label.into(), value }
    }
}

impl<T: Clone + PartialEq + 'static> SearchableListItem for Choice<T> {
    type Value = T;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &T {
        &self.value
    }
}

pub type ChoiceSelect<T> = Entity<SelectState<Vec<Choice<T>>>>;

/// Every value of `T` with its label, in [`Keyed::ALL`]'s order: the one
/// list of labels the sidebar's dropdowns and the preferences share.
pub fn choices_of<T: Keyed>(words: &DesktopWords) -> Vec<Choice<T>> {
    T::ALL.iter().map(|&value| Choice::new(value.label(words), value)).collect()
}

/// A dropdown over `choices` with `selected` committed (the first entry if
/// `selected` is not among them).
pub fn choice_select<T: Clone + PartialEq + 'static>(
    choices: Vec<Choice<T>>,
    selected: &T,
    window: &mut Window,
    cx: &mut App,
) -> ChoiceSelect<T> {
    let index = choices.iter().position(|c| &c.value == selected).unwrap_or(0);
    let index = gpui_kit::component::IndexPath::default().row(index);
    cx.new(|cx| SelectState::new(choices, Some(index), window, cx))
}

/// The committed value. Not the list cursor (`selected_index`), which
/// moves while the popup is open.
pub fn chosen<T: Clone + PartialEq + 'static>(select: &ChoiceSelect<T>, cx: &App) -> Option<T> {
    select.read(cx).selected_value().cloned()
}

/// Replace the entries, keeping the committed value if still offered, else
/// the first; `set_items` alone leaves a selection on a removed entry.
pub fn set_choices<T: Clone + PartialEq + 'static>(
    select: &ChoiceSelect<T>,
    choices: Vec<Choice<T>>,
    window: &mut Window,
    cx: &mut App,
) {
    select.update(cx, |state, cx| {
        let current = state.selected_value().cloned();
        let keep = current.filter(|v| choices.iter().any(|c| &c.value == v));
        let fallback = choices.first().map(|c| c.value.clone());
        state.set_items(choices, window, cx);
        if let Some(value) = keep.or(fallback) {
            state.set_selected_value(&value, window, cx);
        }
    });
}
