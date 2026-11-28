//! The Library's Recently Deleted (ADR-140): the left bar's last entry.
//! Shown, it replaces the grid with what deletes put there, newest first:
//! thumbnail, what it took, days left and size. Restore puts the selection
//! back; Empty Recently Deleted… asks, then removes all for good. Deleting
//! never asks: this list and the notice's Undo bring things back.

use std::collections::HashMap;

use deptheos_app::library::history::{DeletedId, DeletedItem};
use deptheos_app::words::units::size_text;

use super::*;
use crate::multi_select::{Click, MultiSelect};

/// What the Library shows in the grid's place.
///
/// Lifecycle: a filter's `Grid`, or `Deleted` holding a [`RecentlyDeleted`]
/// (reading or listed), changed only by [`Library::set_page`], and reread
/// when [`Renders::trash_changes`] moves (ADR-140).
///
/// ```text
/// Grid(filter) ──a filter──▶ Grid(that filter)
/// Grid ──Recently Deleted's row──▶ Deleted(Reading { list: None }) ──read──▶ Deleted(Listed)
///                                     ▲                                        │
///                                     └──── a delete, restore or Empty ────────┘ (Reading { list: Some }, the list kept on screen)
/// Deleted ──a filter──▶ Grid(that filter)
/// Deleted ──leaving the Library──▶ Grid(All)
/// ```
pub(super) enum Page {
    Grid(Filter),
    Deleted(RecentlyDeleted),
}

impl Page {
    /// The filter stacks are counted by: All behind Recently Deleted.
    pub(super) fn filter(&self) -> Filter {
        match self {
            Page::Grid(filter) => *filter,
            Page::Deleted(_) => Filter::All,
        }
    }

    fn list(&self) -> Option<&DeletedList> {
        match self {
            Page::Grid(_) => None,
            Page::Deleted(deleted) => deleted.list(),
        }
    }

    fn list_mut(&mut self) -> Option<&mut DeletedList> {
        match self {
            Page::Grid(_) => None,
            Page::Deleted(deleted) => deleted.list_mut(),
        }
    }
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Page::Grid(filter) => write!(f, "Grid({filter:?})"),
            Page::Deleted(deleted) => write!(f, "Deleted({deleted:?})"),
        }
    }
}

/// Recently Deleted, shown: being read, or listed. The read is the state's
/// own task, so one that lands is the latest; thumbnails are held only
/// while shown.
pub(super) enum RecentlyDeleted {
    Reading {
        /// The list shown meanwhile, after a change.
        list: Option<DeletedList>,
        /// [`Renders::trash_changes`] when it was asked for.
        changes: u64,
        _read: Task<()>,
    },
    Listed {
        list: DeletedList,
        changes: u64,
    },
}

/// The records listed, the ones selected and their thumbnails.
pub(super) struct DeletedList {
    items: Vec<DeletedItem>,
    selected: MultiSelect<DeletedId>,
    /// Thumbnails once read; none where missing (failed stacks) or unreadable.
    thumbs: HashMap<DeletedId, Option<Arc<RenderImage>>>,
    /// The thumbnails being read, for the records that had none.
    _thumbs: Option<Task<()>>,
}

impl DeletedList {
    fn size(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }
}

impl RecentlyDeleted {
    fn list(&self) -> Option<&DeletedList> {
        match self {
            Self::Reading { list, .. } => list.as_ref(),
            Self::Listed { list, .. } => Some(list),
        }
    }

    fn list_mut(&mut self) -> Option<&mut DeletedList> {
        match self {
            Self::Reading { list, .. } => list.as_mut(),
            Self::Listed { list, .. } => Some(list),
        }
    }

    fn changes(&self) -> u64 {
        match self {
            Self::Reading { changes, .. } | Self::Listed { changes, .. } => *changes,
        }
    }
}

impl std::fmt::Debug for RecentlyDeleted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = |list: Option<&DeletedList>| list.map_or(0, |l| l.items.len());
        match self {
            Self::Reading { list, changes, .. } => {
                write!(f, "Reading(after {changes}, {} shown)", count(list.as_ref()))
            }
            Self::Listed { list, changes } => write!(
                f,
                "Listed(after {changes}, {} records, {} selected)",
                list.items.len(),
                list.selected.len()
            ),
        }
    }
}

impl Library {
    /// The only way [`Self::page`] changes; thumbnails no longer held leave
    /// the atlas.
    pub(super) fn set_page(&mut self, next: Page, cx: &mut Context<Self>) {
        tracing::debug!("library page: {:?} -> {next:?}", self.page);
        let left = std::mem::replace(&mut self.page, next);
        if left.filter() != self.page.filter() {
            self.grid_changed(cx);
        }
        let kept: Vec<&Arc<RenderImage>> =
            self.page.list().into_iter().flat_map(|l| l.thumbs.values().flatten()).collect();
        let dropped: Vec<Arc<RenderImage>> = left
            .list()
            .into_iter()
            .flat_map(|l| l.thumbs.values().flatten())
            .filter(|image| !kept.iter().any(|k| Arc::ptr_eq(k, image)))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            cx.defer(move |cx| {
                for image in dropped {
                    cx.drop_image(image, None);
                }
            });
        }
        cx.notify();
    }

    /// Whether Recently Deleted is shown in the grid's place.
    pub fn is_showing_deleted(&self) -> bool {
        matches!(self.page, Page::Deleted(_))
    }

    /// Its row in the left bar: show Recently Deleted, read afresh.
    pub fn show_deleted(&mut self, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return;
        }
        self.close_preview(cx);
        self.read_deleted(None, cx);
    }

    /// The Library left: back to all stacks, the list let go.
    pub fn hide_deleted(&mut self, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            self.set_page(Page::Grid(Filter::All), cx);
        }
    }

    /// Read the list, keeping `list` on screen meanwhile.
    fn read_deleted(&mut self, list: Option<DeletedList>, cx: &mut Context<Self>) {
        let renders = self.renders.read(cx);
        let changes = renders.trash_changes();
        let read = renders.read_deleted();
        let task = cx.spawn(async move |this, cx| {
            let items = read.await;
            this.update(cx, |this, cx| this.deleted_read(items, cx)).ok();
        });
        self.set_page(Page::Deleted(RecentlyDeleted::Reading { list, changes, _read: task }), cx);
    }

    /// A delete, restore or Empty landed since the read: read again.
    pub(super) fn deleted_maybe_changed(&mut self, cx: &mut Context<Self>) {
        let now = self.renders.read(cx).trash_changes();
        let Page::Deleted(deleted) = &self.page else { return };
        if deleted.changes() == now {
            return;
        }
        // Taken out to be kept on screen: `set_page` then holds it again.
        let list = match std::mem::replace(&mut self.page, Page::Grid(Filter::All)) {
            Page::Deleted(RecentlyDeleted::Reading { list, .. }) => list,
            Page::Deleted(RecentlyDeleted::Listed { list, .. }) => Some(list),
            Page::Grid(_) => None,
        };
        self.read_deleted(list, cx);
    }

    /// The list was read: keep the selection and thumbnails of records still
    /// in it, and read the missing thumbnails.
    fn deleted_read(&mut self, items: Result<Vec<DeletedItem>, String>, cx: &mut Context<Self>) {
        let Page::Deleted(RecentlyDeleted::Reading { list, changes, .. }) = &self.page else {
            return;
        };
        let changes = *changes;
        let items = items.unwrap_or_else(|e| {
            tracing::warn!("couldn't read Recently Deleted: {e}");
            Vec::new()
        });
        // `set_page` drops the thumbnails not carried over.
        let (mut selected, mut thumbs) = match list {
            Some(old) => (old.selected.clone(), old.thumbs.clone()),
            None => (MultiSelect::none(), HashMap::new()),
        };
        let listed = |id: &DeletedId| items.iter().any(|i| i.id == *id);
        selected.retain(|id| listed(id));
        thumbs.retain(|id, _| listed(id));
        let missing: Vec<DeletedId> =
            items.iter().map(|i| i.id.clone()).filter(|id| !thumbs.contains_key(id)).collect();
        let reading = (!missing.is_empty()).then(|| {
            let read = self.renders.read(cx).read_deleted_thumbnails(missing);
            cx.spawn(async move |this, cx| {
                let read = read.await;
                this.update(cx, |this, cx| this.deleted_thumbs_read(read, cx)).ok();
            })
        });
        let list = DeletedList { items, selected, thumbs, _thumbs: reading };
        self.set_page(Page::Deleted(RecentlyDeleted::Listed { list, changes }), cx);
    }

    /// The deleted records' thumbnails held.
    pub(super) fn deleted_thumbs_mut(&mut self) -> impl Iterator<Item = &mut Arc<RenderImage>> {
        self.page.list_mut().into_iter().flat_map(|l| l.thumbs.values_mut().flatten())
    }

    /// Thumbnails read: taken by the records still listed without one.
    fn deleted_thumbs_read(
        &mut self,
        read: Vec<(DeletedId, Option<CpuImage>)>,
        cx: &mut Context<Self>,
    ) {
        let shown = self.shown;
        let Some(list) = self.page.list_mut() else { return };
        for (id, image) in read {
            if list.items.iter().any(|i| i.id == id) && !list.thumbs.contains_key(&id) {
                list.thumbs.insert(id, image.map(|image| render_srgb(image, shown)));
            }
        }
        cx.notify();
    }

    /// A click on a record, by the Finder's rules ([`MultiSelect`]).
    fn click_deleted(&mut self, id: &DeletedId, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(list) = self.page.list_mut() else { return };
        let order: Vec<DeletedId> = list.items.iter().map(|i| i.id.clone()).collect();
        list.selected.click(id, Click::of(modifiers), &order);
        tracing::debug!("library recently deleted: {} selected", list.selected.len());
        cx.notify();
    }

    /// ⌘A in Recently Deleted: every record; Esc (`all` false): none.
    pub(super) fn select_all_deleted(&mut self, all: bool, cx: &mut Context<Self>) {
        let Some(list) = self.page.list_mut() else { return };
        list.selected = match all {
            true => MultiSelect::of(list.items.iter().map(|i| i.id.clone()).collect()),
            false => MultiSelect::none(),
        };
        cx.notify();
    }

    /// The records selected, in the list's order.
    fn selected_deleted(&self) -> Vec<DeletedId> {
        let Some(list) = self.page.list() else { return Vec::new() };
        list.selected.in_order(list.items.iter().map(|i| &i.id))
    }

    /// Restore: put the records selected back.
    pub fn restore_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_deleted();
        self.renders.update(cx, |r, cx| r.restore(ids, window, cx));
    }

    /// Empty Recently Deleted…: ask, naming the count and space freed, then
    /// remove all for good.
    pub fn ask_empty_deleted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.page.list().filter(|l| !l.items.is_empty()) else { return };
        let size = size_text(list.size(), cx.wording().shared());
        let (title, text, button) =
            history::empty_deleted_question(crate::words::of(cx), list.items.len(), &size);
        let renders = self.renders.downgrade();
        let confirm = move |window: &mut Window, cx: &mut gpui::App| {
            renders.update(cx, |r, cx| r.empty_deleted(window, cx)).ok();
        };
        crate::form::ask_destructive(title, text, button, confirm, window, cx);
    }

    /// The titles of the records listed, newest first, as their rows say.
    #[cfg(test)]
    pub fn deleted_titles(&self, cx: &gpui::App) -> Option<Vec<String>> {
        let list = match &self.page {
            Page::Deleted(RecentlyDeleted::Listed { list, .. }) => list,
            _ => return None,
        };
        let untitled = say!(cx, "app-untitled-stack");
        let w = crate::words::of(cx);
        Some(list.items.iter().map(|i| history::deleted_title(w, i, &untitled)).collect())
    }

    /// Its row at the end of the left bar, with how many records it holds.
    pub(super) fn render_deleted_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let count = self.renders.read(cx).trash().map(|(n, _)| n.to_string());
        h_flex()
            .id("show-Recently Deleted")
            .debug_selector(|| "library-show-deleted".into())
            .mt_3()
            .px_2()
            .py_1()
            .gap_1p5()
            .rounded_md()
            .text_sm()
            .pickable(self.is_showing_deleted(), true, cx)
            .child(Icon::new(IconName::Trash).small().text_color(muted))
            .child(div().flex_1().child(say!(cx, "library-recently-deleted")))
            .children(count.map(|n| div().text_xs().text_color(muted).child(n)))
            .on_click(cx.listener(|this, _, _, cx| this.show_deleted(cx)))
            .into_any_element()
    }

    /// Recently Deleted in the grid's place: title with contents, Restore,
    /// Empty Recently Deleted…, then a tile per record.
    pub(super) fn render_deleted(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let list = self.page.list();
        let w = cx.wording().clone();
        let summary = list.map(|l| {
            let size = size_text(l.size(), w.shared());
            history::deleted_summary(w.shared(), l.items.len(), &size)
        });
        let can_restore = self.renders.read(cx).can_restore();
        let selected = list.map_or(0, |l| l.selected.len());
        let empty = list.is_none_or(|l| l.items.is_empty());
        let header = h_flex()
            .gap_2()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(say!(cx, "library-recently-deleted")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(summary.unwrap_or_else(|| say!(cx, "library-deleted-reading"))),
                    ),
            )
            .child(
                div().debug_selector(|| "deleted-restore".into()).child(
                    Button::new("deleted-restore")
                        .label(say!(cx, "library-deleted-restore"))
                        .small()
                        .disabled(selected == 0 || !can_restore)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.restore_selected(window, cx)),
                        ),
                ),
            )
            .child(
                div().debug_selector(|| "deleted-empty".into()).child(
                    Button::new("deleted-empty")
                        .label(say!(cx, "library-deleted-empty"))
                        .small()
                        .outline()
                        .disabled(empty || !can_restore)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.ask_empty_deleted(window, cx)),
                        ),
                ),
            );
        let now = SystemTime::now();
        let tiles: Vec<AnyElement> = list
            .map(|l| {
                l.items.iter().map(|item| self.render_deleted_tile(l, item, now, cx)).collect()
            })
            .unwrap_or_default();
        v_flex()
            .id("library-deleted")
            .debug_selector(|| "library-deleted".into())
            .size_full()
            .px_6()
            .pt_4()
            .gap_4()
            // Tiles keep their clicks; empty space selects nothing.
            .on_click(cx.listener(|this, _, _, cx| this.select_all_deleted(false, cx)))
            .child(header)
            .child(
                div()
                    .id("library-deleted-tiles")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(h_flex().flex_wrap().gap_4().px(px(TILE_INSET)).pb_6().children(tiles)),
            )
            .into_any_element()
    }

    /// A record's tile: thumbnail, what it took, days left, size; Restore in
    /// its menu.
    fn render_deleted_tile(
        &self,
        list: &DeletedList,
        item: &DeletedItem,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let selected = list.selected.contains(&item.id);
        let (ring, muted) = (theme.ring, theme.muted_foreground);
        let untitled = say!(cx, "app-untitled-stack");
        let title = history::deleted_title(crate::words::of(cx), item, &untitled);
        let days = history::deleted_days_left(crate::words::of(cx), item, now);
        let size = size_text(item.size, cx.wording().shared());
        let detail = say!(cx, "library-deleted-detail", days = days, size = size);
        let thumb = div()
            .w(px(TILE_WIDTH))
            .h(px(TILE_THUMB_HEIGHT))
            .rounded_md()
            .overflow_hidden()
            .border_2()
            .border_color(if selected { ring } else { gpui::transparent_black() })
            .bg(theme.muted);
        let thumb = match list.thumbs.get(&item.id) {
            Some(Some(image)) => {
                thumb.child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
            }
            // Only failed stacks: nothing to show.
            Some(None) if item.renders.is_empty() => thumb.child(tile_placeholder(
                IconName::TriangleAlert,
                say!(cx, "library-tile-failed"),
                theme.danger,
            )),
            _ => thumb,
        };
        let id = item.id.clone();
        let menu_id = item.id.clone();
        let this = cx.weak_entity();
        let selector = format!("library-deleted-{title}");
        v_flex()
            .id(SharedString::from(format!("deleted-{}", item.id.as_str())))
            .debug_selector(move || selector.clone())
            .w(px(TILE_WIDTH + 2.0 * TILE_INSET))
            .m(px(-TILE_INSET))
            .p(px(TILE_INSET))
            .rounded(theme.radius_lg)
            .gap_0p5()
            .pickable(selected, true, cx)
            .child(thumb)
            .child(div().mt_1().text_sm().font_weight(FontWeight::MEDIUM).truncate().child(title))
            .child(div().text_xs().text_color(muted).truncate().child(detail))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.click_deleted(&id, event.modifiers(), cx);
            }))
            .context_menu(move |menu, _, cx| {
                // A right-click outside the selection selects that record.
                let can_restore = this
                    .update(cx, |this, cx| {
                        let held = this.page.list().is_some_and(|l| l.selected.contains(&menu_id));
                        if !held {
                            this.click_deleted(&menu_id, Modifiers::none(), cx);
                        }
                        this.renders.read(cx).can_restore()
                    })
                    .unwrap_or(false);
                let restore =
                    entity_item(say!(cx, "library-deleted-restore"), &this, |l, window, cx| {
                        l.restore_selected(window, cx)
                    })
                    .disabled(!can_restore);
                menu.item(restore)
            })
            .into_any_element()
    }
}
