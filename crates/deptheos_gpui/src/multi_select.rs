//! A multiple selection with the Finder's rules, shared by the Library's
//! grid, the frame list and Recently Deleted: click selects one, ⌘-click
//! toggles one (removing the last leaves none), ⇧-click ranges from the anchor.

use gpui::Modifiers;

/// How an item was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Alone: that item is the selection.
    Plain,
    /// With ⌘: the item is added, or taken out.
    Toggle,
    /// With ⇧: the items from the anchor to it, in the list's order.
    Range,
}

impl Click {
    /// The click `modifiers` make: ⇧ before ⌘, as the Finder has it.
    pub fn of(modifiers: Modifiers) -> Self {
        if modifiers.shift {
            Click::Range
        } else if modifiers.secondary() {
            Click::Toggle
        } else {
            Click::Plain
        }
    }
}

/// The selected items in selection order, and a range's anchor; `None` is
/// nothing selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiSelect<Id>(Option<Picked<Id>>);

/// Never empty; the anchor is one of `ids`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Picked<Id> {
    ids: Vec<Id>,
    anchor: Id,
}

impl<Id> Default for MultiSelect<Id> {
    fn default() -> Self {
        Self(None)
    }
}

impl<Id: Clone + PartialEq> MultiSelect<Id> {
    /// Nothing selected.
    pub fn none() -> Self {
        Self(None)
    }

    /// `id` alone, the anchor.
    pub fn one(id: Id) -> Self {
        Self(Some(Picked { ids: vec![id.clone()], anchor: id }))
    }

    /// `ids`, the first the anchor: nothing when there are none.
    pub fn of(ids: Vec<Id>) -> Self {
        match ids.first().cloned() {
            Some(anchor) => Self(Some(Picked { ids, anchor })),
            None => Self(None),
        }
    }

    /// A click on `id` among `order`; a range without anchor, or with anchor
    /// or `id` outside `order`, selects `id` alone.
    pub fn click(&mut self, id: &Id, click: Click, order: &[Id]) {
        let next = match (self.0.take(), click) {
            (_, Click::Plain) | (None, _) => Self::one(id.clone()),
            (Some(Picked { mut ids, anchor }), Click::Toggle) => {
                match ids.iter().position(|s| s == id) {
                    Some(at) => {
                        ids.remove(at);
                        // The anchor stays while selected, else the last
                        // selected left replaces it.
                        let anchor = match ids.contains(&anchor) {
                            true => Some(anchor),
                            false => ids.last().cloned(),
                        };
                        Self(anchor.map(|anchor| Picked { ids, anchor }))
                    }
                    None => {
                        ids.push(id.clone());
                        Self(Some(Picked { ids, anchor: id.clone() }))
                    }
                }
            }
            (Some(Picked { anchor, .. }), Click::Range) => {
                let at = |s: &Id| order.iter().position(|o| o == s);
                match (at(&anchor), at(id)) {
                    (Some(a), Some(b)) => {
                        Self(Some(Picked { ids: order[a.min(b)..=a.max(b)].to_vec(), anchor }))
                    }
                    _ => Self::one(id.clone()),
                }
            }
        };
        *self = next;
    }

    /// The items selected, in the order they were selected.
    pub fn ids(&self) -> &[Id] {
        self.0.as_ref().map_or(&[], |p| &p.ids)
    }

    /// The items selected in `order`'s order, leaving out those not in it.
    pub fn in_order<'a>(&self, order: impl IntoIterator<Item = &'a Id>) -> Vec<Id>
    where
        Id: 'a,
    {
        order.into_iter().filter(|id| self.contains(id)).cloned().collect()
    }

    /// Where a range starts: the item last clicked without ⇧.
    pub fn anchor(&self) -> Option<&Id> {
        self.0.as_ref().map(|p| &p.anchor)
    }

    /// The item selected last.
    pub fn last(&self) -> Option<&Id> {
        self.ids().last()
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.ids().contains(id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn len(&self) -> usize {
        self.ids().len()
    }

    /// Keep the items `keep` allows as items leave; a removed anchor is
    /// replaced by the last selected left.
    pub fn retain(&mut self, keep: impl Fn(&Id) -> bool) {
        let Some(Picked { mut ids, anchor }) = self.0.take() else { return };
        ids.retain(|id| keep(id));
        let anchor = match ids.contains(&anchor) {
            true => Some(anchor),
            false => ids.last().cloned(),
        };
        self.0 = anchor.map(|anchor| Picked { ids, anchor });
    }
}

#[cfg(test)]
mod tests {
    use super::{Click, MultiSelect};

    const ORDER: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

    /// `ids` (in selection order) with `anchor`; "" is nothing.
    fn sel(ids: &str, anchor: char) -> MultiSelect<char> {
        let mut s = MultiSelect::of(ids.chars().collect());
        if let Some(p) = s.0.as_mut() {
            p.anchor = anchor;
        }
        s
    }

    /// A rule: selection before, item clicked and how, selection after.
    type Case = (&'static str, MultiSelect<char>, char, Click, MultiSelect<char>);

    #[test]
    fn clicks_follow_the_finder() {
        use Click::{Plain, Range, Toggle};
        let none = || MultiSelect::<char>::none();
        let cases: Vec<Case> = vec![
            ("a click selects one", none(), 'c', Plain, sel("c", 'c')),
            ("a click replaces the selection", sel("abc", 'a'), 'e', Plain, sel("e", 'e')),
            (
                "a click on a selected item selects it alone",
                sel("abc", 'a'),
                'b',
                Plain,
                sel("b", 'b'),
            ),
            ("⌘ with nothing selects one", none(), 'c', Toggle, sel("c", 'c')),
            ("⌘ adds, the anchor moving to it", sel("b", 'b'), 'e', Toggle, sel("be", 'e')),
            ("⌘ takes one out, the anchor kept", sel("bce", 'b'), 'c', Toggle, sel("be", 'b')),
            (
                "⌘ on the anchor: the last one left anchors",
                sel("bce", 'c'),
                'c',
                Toggle,
                sel("be", 'e'),
            ),
            ("⌘ on the last one selected empties it", sel("c", 'c'), 'c', Toggle, none()),
            ("⇧ with nothing selects one", none(), 'd', Range, sel("d", 'd')),
            ("⇧ selects down from the anchor", sel("b", 'b'), 'e', Range, sel("bcde", 'b')),
            ("⇧ selects up from the anchor", sel("e", 'e'), 'b', Range, sel("bcde", 'e')),
            ("⇧ again replaces the range", sel("bcde", 'b'), 'c', Range, sel("bc", 'b')),
            ("⇧ after ⌘ ranges from the item ⌘ added", sel("ae", 'e'), 'f', Range, sel("ef", 'e')),
            ("⇧ on the anchor selects it alone", sel("abc", 'a'), 'a', Range, sel("a", 'a')),
        ];
        for (rule, before, id, click, after) in cases {
            let mut s = before;
            s.click(&id, click, &ORDER);
            assert_eq!(s, after, "{rule}");
        }
    }

    /// A range from an anchor no longer listed selects the item alone.
    #[test]
    fn a_range_from_an_anchor_gone_selects_one() {
        let mut s = sel("z", 'z');
        s.click(&'c', Click::Range, &ORDER);
        assert_eq!(s, sel("c", 'c'));
    }

    /// ⇧ wins over ⌘, as the Finder's ⇧⌘-click ranges.
    #[test]
    fn shift_before_command() {
        let both =
            gpui::Modifiers { shift: true, platform: true, control: true, ..Default::default() };
        assert_eq!(Click::of(both), Click::Range);
        assert_eq!(Click::of(gpui::Modifiers::secondary_key()), Click::Toggle);
        assert_eq!(Click::of(gpui::Modifiers::none()), Click::Plain);
    }

    /// Items leaving the list leave the selection; a leaving anchor is
    /// replaced by the last left, or nothing.
    #[test]
    fn items_gone_leave_the_selection() {
        let mut s = sel("bdf", 'd');
        s.retain(|id| *id != 'd');
        assert_eq!(s, sel("bf", 'f'));
        s.retain(|_| false);
        assert!(s.is_empty());
        assert_eq!(s.anchor(), None);
    }

    /// Read back in the list's order, whatever order they were selected in.
    #[test]
    fn reads_in_the_lists_order() {
        let s = sel("eb", 'b');
        assert_eq!(s.in_order(&ORDER), ['b', 'e']);
        assert_eq!(s.ids(), ['e', 'b']);
        assert_eq!(s.last(), Some(&'b'));
        assert_eq!(s.len(), 2);
    }
}
