//! The Library's inspector, its right-hand panel
//! (`Library::render_details`), and its per-render rows.

use super::layout::{Kind, folder_text, frames_text, renders_text};
use super::*;
use crate::renders::show_frames_label;
use gpui::{Div, Stateful};

impl Library {
    /// The selection: one stack with its renders, or how many are selected.
    pub(super) fn render_details(&self, grid: &Grid, cx: &mut Context<Self>) -> AnyElement {
        let pane = details_pane(cx);
        let selected = self.selected_in(grid);
        let stack = match &selected[..] {
            [] => {
                let none = say!(cx, "library-details-none");
                let muted = cx.theme().muted_foreground;
                return pane
                    .child(div().text_sm().text_color(muted).child(none))
                    .into_any_element();
            }
            [stack] => stack.clone(),
            several => return self.details_of_several(pane, several, cx),
        };
        let Some(tile) = grid.tile_of(&stack).cloned() else { return pane.into_any_element() };
        match tile.kind {
            Kind::Unstacked { frames, folder } => {
                self.details_of_unstacked(pane, &stack, tile.title, frames, folder, cx)
            }
            Kind::Failed { failure } => self.details_of_failed(pane, &stack, &failure, cx),
            Kind::Renders { .. } => self.details_of_renders(pane, grid, &stack, tile.title, cx),
        }
    }

    /// Two or more stacks: how many, their renders and their size, and
    /// Delete.
    fn details_of_several(
        &self,
        pane: Stateful<Div>,
        several: &[StackId],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let renders = self.renders.read(cx);
        let ids: Vec<&RenderId> = renders
            .renders()
            .iter()
            .filter(|r| several.contains(&r.stack))
            .map(|r| &r.id)
            .collect();
        let size = renders.size_text(ids.iter().copied(), cx.wording()).filter(|_| !ids.is_empty());
        let about = renders_text(cx.wording(), ids.len(), size);
        let stacks = several.to_vec();
        pane.child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(say!(
            cx,
            "library-details-selected",
            n = several.len()
        )))
        .child(div().text_xs().text_color(muted).child(about))
        .child(
            Button::new("library-delete-selected")
                .label(say!(cx, "library-details-delete"))
                .small()
                .outline()
                .w_full()
                .disabled(!self.can_delete(cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.delete_stacks(stacks.clone(), window, cx)
                })),
        )
        .into_any_element()
    }

    /// A stack never stacked: its frames and folder, Show Frames and Open.
    fn details_of_unstacked(
        &self,
        pane: Stateful<Div>,
        stack: &StackId,
        title: String,
        frames: usize,
        folder: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = cx.wording().clone();
        let muted = cx.theme().muted_foreground;
        pane.child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_xs().text_color(muted).child(say!(w, "library-details-unstacked")))
            .child(
                v_flex()
                    .gap_1p5()
                    .child(detail_row(say!(w, "library-details-frames"), frames.to_string(), muted))
                    .child(long_row(
                        "library-folder",
                        say!(w, "library-details-folder"),
                        folder,
                        muted,
                    )),
            )
            .child(show_frames_button(stack, &w, cx))
            .child(open_button(stack, say!(w, "library-details-open"), cx))
            .into_any_element()
    }

    /// A batch stack that failed: when it was tried, its frames, folder and
    /// batch, why it failed, Show Frames and Open to Retry.
    fn details_of_failed(
        &self,
        pane: Stateful<Div>,
        stack: &StackId,
        failure: &RenderId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = cx.wording().clone();
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let renders = self.renders.read(cx);
        let Some(f) = renders.failure(failure) else { return pane.into_any_element() };
        let rows = v_flex()
            .gap_1p5()
            .child(detail_row(say!(w, "library-details-tried"), made(&w, f.created), muted))
            .child(detail_row(
                say!(w, "library-details-frames"),
                frames_text(&w, f.frames.len(), f.frames.len()),
                muted,
            ))
            .child(long_row(
                "library-folder",
                say!(w, "library-details-folder"),
                folder_text(f.frames.first().map(PathBuf::as_path)),
                muted,
            ))
            .child(long_row(
                "library-batch",
                say!(w, "library-details-batch"),
                f.batch.label.text(w.shared()),
                muted,
            ));
        let (label, error) = (f.label.clone(), f.error.clone());
        pane.child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(label))
            .child(div().text_xs().text_color(danger).child(say!(
                w,
                "library-details-failed",
                error = error.as_str()
            )))
            .child(rows)
            .child(show_frames_button(stack, &w, cx))
            .child(open_button(stack, say!(w, "library-details-open-to-retry"), cx))
            .into_any_element()
    }

    /// A stack with renders: the render shown's picture, name, renders and
    /// size, newest, frames and folder, kept working version, Rename, Export,
    /// Show Frames, and every render.
    fn details_of_renders(
        &self,
        pane: Stateful<Div>,
        grid: &Grid,
        stack: &StackId,
        title: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = cx.wording().clone();
        let theme = cx.theme();
        let (muted, border, blank) = (theme.muted_foreground, theme.border, theme.muted);
        let Some(shown) = self.details_render(grid, cx) else { return pane.into_any_element() };
        let renders = self.renders.read(cx);
        let Some(r) = renders.find(&shown).cloned() else {
            return pane.into_any_element();
        };
        // Every render of the stack, newest first, whatever the filter.
        let mut of_stack: Vec<&history::Render> =
            renders.renders().iter().filter(|o| o.stack == *stack).collect();
        of_stack.sort_by_key(|o| std::cmp::Reverse(o.number));
        let ids: Vec<&RenderId> = of_stack.iter().map(|o| &o.id).collect();
        let about = renders_text(&w, of_stack.len(), renders.size_text(ids.iter().copied(), &w));
        let stacked = r.included.iter().filter(|i| **i).count();
        let kept = self.kept_text(&w, renders, stack);
        let edited = crate::params::edited_color(cx);
        let rows = v_flex()
            .gap_1p5()
            .child(detail_row(
                say!(w, "library-details-newest"),
                made(&w, of_stack.first().map_or(r.created, |o| o.created)),
                muted,
            ))
            .child(detail_row(
                say!(w, "library-details-frames"),
                frames_text(&w, stacked, r.frames.len()),
                muted,
            ))
            .child(long_row(
                "library-folder",
                say!(w, "library-details-folder"),
                folder_text(r.frames.first().map(PathBuf::as_path)),
                muted,
            ));
        let colors = (muted, blank);
        let (this, store) = (cx.weak_entity(), self.renders.downgrade());
        let list: Vec<AnyElement> = of_stack
            .iter()
            .map(|o| {
                let thumbnail = renders.thumbnail(&o.id);
                let selected = self.selected_render() == Some(&o.id);
                let links = (this.clone(), store.clone());
                render_row(o, thumbnail, selected, colors, links, &w, cx)
            })
            .collect();
        let (rename_stack, export_id) = (stack.clone(), shown);
        // The whole picture, on the backdrop the app keeps behind photos:
        // a portrait render isn't cut to the box's landscape shape.
        let picture = self.cover_image(&r.id, cx);
        pane.child(
            div()
                .id("library-details-cover")
                .debug_selector(|| "library-details-cover".into())
                .w_full()
                .h(px(150.0))
                .flex_shrink_0()
                .rounded_md()
                .overflow_hidden()
                .bg(gpui::rgb(crate::brand::VIEWER_BACKGROUND))
                .when_some(picture, |el, image| {
                    el.child(img(image).size_full().object_fit(ObjectFit::Contain))
                }),
        )
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(muted).child(about)),
        )
        .child(rows)
        .when_some(kept, |el, (heading, text)| {
            el.child(
                v_flex()
                    .p_2p5()
                    .gap_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(div().size(px(6.0)).rounded_full().bg(edited))
                            .child(heading),
                    )
                    .child(div().text_xs().child(text)),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("library-rename")
                        .label(say!(w, "library-details-rename"))
                        .small()
                        .outline()
                        .flex_1()
                        .tooltip(say!(w, "library-details-rename-tip"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.rename_stack(rename_stack.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new("library-export")
                        .label(say!(w, "library-details-export", number = r.number.to_string()))
                        .small()
                        .outline()
                        .flex_1()
                        .tooltip(say!(w, "library-details-export-tip"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(LibraryEvent::Export(export_id.clone()))
                        })),
                ),
        )
        .child(show_frames_button(stack, &w, cx))
        .child(
            h_flex()
                .pt_1()
                .gap_1()
                .items_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child(Icon::new(crate::AppIcon::Render).xsmall())
                .child(say!(w, "library-details-renders-heading")),
        )
        .child(v_flex().gap_1().children(list))
        .into_any_element()
    }

    /// Show Frames in Finder for `stack`: its first frame, in the file
    /// manager.
    pub(super) fn show_frames(&self, stack: &StackId, cx: &mut Context<Self>) {
        if let Some(frame) = self.first_frame(stack, cx) {
            cx.reveal_path(&frame);
        }
    }
}

/// The inspector's panel, scrolling, that the selection's details fill.
fn details_pane(cx: &gpui::App) -> Stateful<Div> {
    let theme = cx.theme();
    v_flex()
        .id("library-details")
        .h_full()
        .w(px(DETAILS_WIDTH))
        .flex_shrink_0()
        .p_4()
        .gap_3()
        .bg(theme.sidebar)
        .border_l_1()
        .border_color(theme.border)
        .overflow_y_scroll()
}

/// A detail: its label, muted, and its value, cut short at the end.
fn detail_row(label: String, value: String, muted: gpui::Hsla) -> Div {
    h_flex()
        .gap_2()
        .text_xs()
        .child(div().w(px(84.0)).flex_shrink_0().text_color(muted).child(label))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(value))
}

/// A detail whose value can be cut short (a folder, a batch's name): the
/// whole of it on hover.
fn long_row(id: &'static str, label: String, value: String, muted: gpui::Hsla) -> Stateful<Div> {
    let tip = SharedString::from(value.clone());
    detail_row(label, value, muted).id(id).tooltip(move |window, cx| {
        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
    })
}

/// When something was made: its day as the Library says it, and the time.
fn made(w: &DesktopWords, t: SystemTime) -> String {
    let local = DateTime::<Local>::from(t);
    let day = day_label(w, local.date_naive(), Local::now().date_naive());
    say!(w, "age-day-time", day = day, time = crate::dates::time(local))
}

/// Show Frames in Finder, for `stack`.
fn show_frames_button(stack: &StackId, w: &DesktopWords, cx: &mut Context<Library>) -> Button {
    let stack = stack.clone();
    Button::new("library-show-frames")
        .debug_selector(|| "library-show-frames".into())
        .label(show_frames_label(w))
        .small()
        .outline()
        .w_full()
        .on_click(cx.listener(move |this, _, _, cx| this.show_frames(&stack, cx)))
}

/// Open, or Open to Retry: `stack` in Stacking.
fn open_button(stack: &StackId, label: String, cx: &mut Context<Library>) -> Button {
    let stack = stack.clone();
    Button::new("library-open-stack")
        .label(label)
        .small()
        .outline()
        .w_full()
        .on_click(cx.listener(move |this, _, _, cx| this.open_stack(&stack, cx)))
}

/// One render of the selected stack, in the inspector: a click selects
/// it, a double-click opens it as stacked, and the rest is in its menu.
fn render_row(
    render: &history::Render,
    thumbnail: Option<Arc<RenderImage>>,
    selected: bool,
    (muted, blank): (gpui::Hsla, gpui::Hsla),
    (this, store): (gpui::WeakEntity<Library>, gpui::WeakEntity<Renders>),
    w: &DesktopWords,
    cx: &gpui::App,
) -> AnyElement {
    let title = render_title(w, render);
    let (id, menu_id, picked) = (render.id.clone(), render.id.clone(), render.picked);
    let stack = render.stack.clone();
    let (opener, picker) = (this.clone(), this.clone());
    let (menu_stack, menu_render) = (stack.clone(), id.clone());
    let selector = format!("library-render-{}", render.number);
    h_flex()
        .id(SharedString::from(format!("library-render-{}", render.id.as_str())))
        .debug_selector(move || selector.clone())
        .gap_2()
        .p_1()
        .rounded_md()
        .pickable(selected, true, cx)
        .child(thumb_box(thumbnail, blank).w(px(54.0)).h(px(38.0)).flex_shrink_0().rounded_sm())
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).truncate().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(render.config.summary(w.shared())),
                ),
        )
        .on_click(move |event: &ClickEvent, _, cx| {
            let clicks = event.click_count();
            opener.update(cx, |l, cx| l.click_render(&stack, &id, clicks, cx)).ok();
        })
        .context_menu(move |menu, _, cx| {
            // A right-click selects the render, as one on a tile does.
            picker.update(cx, |l, cx| l.click_render(&menu_stack, &menu_render, 1, cx)).ok();
            let can_delete = store.upgrade().is_some_and(|r| r.read(cx).can_delete());
            let w = cx.wording();
            let emit = |label: String, event: fn(RenderId) -> LibraryEvent| {
                let id = menu_id.clone();
                entity_item(label, &this, move |_, _, cx| cx.emit(event(id.clone())))
            };
            let star = {
                let id = menu_id.clone();
                let label = if picked { say!(w, "render-unstar") } else { say!(w, "render-star") };
                entity_item(label, &store, move |r, window, cx| r.toggle_pick(&id, window, cx))
            };
            let delete = {
                let id = menu_id.clone();
                entity_item(say!(w, "render-delete"), &store, move |r, window, cx| {
                    if r.can_delete() {
                        r.delete(vec![id.clone()], window, cx)
                    }
                })
                .disabled(!can_delete)
            };
            menu.item(emit(say!(w, "library-menu-open"), LibraryEvent::Open))
                .item(emit(say!(w, "library-menu-export"), LibraryEvent::Export))
                .separator()
                .item(star)
                .separator()
                .item(delete)
        })
        .into_any_element()
}
