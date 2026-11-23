//! Dialog and panel pieces: section heading, note, radio row, segmented
//! control, disclosure row, edited dot, and the Cancel + primary footer.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyElement, AnyView, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, Hsla,
    SharedString, Stateful, Window, div, point, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};

use crate::look::Pickable as _;
use crate::words::say;
use gpui_kit::component::button::Button;
use gpui_kit::component::h_flex;
use gpui_kit::component::radio::{Radio, RadioGroup};

/// A section's heading: "Direction", "After stacking".
pub fn heading(text: impl Into<SharedString>) -> Div {
    div().text_sm().font_weight(FontWeight::MEDIUM).child(text.into())
}

/// Small muted text under a control.
pub fn note(text: impl Into<SharedString>, muted: Hsla) -> Div {
    div().text_xs().text_color(muted).child(text.into())
}

/// A radio per value of `all`, worded by `label`: `selected` checked, those
/// `enabled` refuses dimmed, a click handing its value to `pick`.
pub fn radio_row<T: Copy + PartialEq + 'static, L: Into<SharedString>>(
    group: RadioGroup,
    all: &'static [T],
    label: impl Fn(T) -> L,
    selected: T,
    enabled: impl Fn(T) -> bool,
    pick: impl Fn(&T, &mut Window, &mut App) + 'static,
) -> RadioGroup {
    // The group numbers its radios itself; the label stands in for an id.
    group
        .children(all.iter().map(|&value| {
            let label: SharedString = label(value).into();
            Radio::new(label.clone()).label(label).disabled(!enabled(value))
        }))
        .selected_index(all.iter().position(|v| *v == selected))
        .on_click(move |ix, window, cx| {
            if let Some(value) = all.get(*ix) {
                pick(value, window, cx);
            }
        })
}

/// Cancel, then the dialog's `primary` button, at the right.
pub fn footer(
    cancel_id: &'static str,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    primary: Button,
) -> Footer {
    Footer { cancel_id, on_cancel: Box::new(on_cancel), primary }
}

/// What Cancel does.
type OnCancel = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// [`footer`]'s row, worded as it is drawn.
#[derive(IntoElement)]
pub struct Footer {
    cancel_id: &'static str,
    on_cancel: OnCancel,
    primary: Button,
}

impl RenderOnce for Footer {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_cancel = self.on_cancel;
        h_flex()
            .gap_2()
            .justify_end()
            .child(
                Button::new(self.cancel_id)
                    .label(say!(cx, "button-cancel"))
                    .on_click(move |event, window, cx| on_cancel(event, window, cx)),
            )
            .child(self.primary)
    }
}

/// Ask before something irreversible: `title` and `description` over a red
/// `act` button ("Delete 3 Renders") that runs `confirm`, and Cancel, the
/// default for both Return and Esc. Opens over any open dialog.
pub fn ask_destructive(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    act: impl Into<SharedString>,
    confirm: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::button::{ButtonVariant, ButtonVariants as _};
    use gpui_kit::component::dialog::DialogFooter;

    let (title, description, act) = (title.into(), description.into(), act.into());
    let cancel_label = SharedString::from(say!(cx, "button-cancel"));
    let confirm = Rc::new(confirm);
    tracing::debug!(%title, "destructive alert: asking");
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let confirm = confirm.clone();
        let act = Button::new("destructive-confirm")
            .label(act.clone())
            .with_variant(ButtonVariant::Danger)
            .on_click(move |_, window, cx| {
                window.close_dialog(cx);
                confirm(window, cx);
            });
        let cancel = Button::new("destructive-cancel")
            .label(cancel_label.clone())
            .primary()
            .on_click(|_, window, cx| window.close_dialog(cx));
        // Return is the dialog's OK, here Cancel: it closes unchanged, like Esc.
        dialog
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .footer(
                DialogFooter::new()
                    .child(div().debug_selector(|| "alert-destructive".into()).child(act))
                    .child(cancel),
            )
            .on_ok(|_, _, _| true)
    });
}

/// The checkbox that shows an export once it is saved.
pub fn reveal_when_done_label(cx: &App) -> String {
    if cfg!(target_os = "macos") {
        say!(cx, "form-reveal-macos")
    } else if cfg!(target_os = "windows") {
        say!(cx, "form-reveal-windows")
    } else {
        say!(cx, "form-reveal-other")
    }
}

/// The dot that marks a changed setting.
pub fn dot(color: Hsla) -> impl IntoElement {
    div().size(px(6.0)).flex_shrink_0().rounded_full().bg(color)
}

/// A disclosure row: chevron, `title`, muted `summary` while shut, and the
/// edited dot in `edited` when something under it changed; a click runs
/// `on_toggle`.
pub fn disclosure(
    id: &'static str,
    title: String,
    summary: String,
    open: bool,
    edited: Option<Hsla>,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .id(id)
        .gap_1p5()
        .px_1()
        .py_0p5()
        .mx(px(-4.0))
        .rounded(cx.theme().radius)
        .items_center()
        .pickable(false, true, cx)
        .text_xs()
        .child(
            Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                .xsmall()
                .text_color(muted),
        )
        .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(muted)
                .when(!open, |el| el.child(summary)),
        )
        .children(edited.map(dot))
        .on_click(on_toggle)
}

/// What a segment's tooltip is built by.
type TooltipBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// One choice of a [`Segmented`] control.
pub struct Segment {
    label: SharedString,
    suffix: Option<AnyElement>,
    disabled: bool,
    tooltip: Option<TooltipBuilder>,
}

impl Segment {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), suffix: None, disabled: false, tooltip: None }
    }

    /// Dimmed, and passed over by a click and the arrow keys.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Drawn after the label: a key cap.
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    pub fn tooltip(mut self, build: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Rc::new(build));
        self
    }
}

/// What a click or an arrow key picks.
type OnPick = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// A segmented control. The selected segment is `surface` with a small
/// shadow on a `surface-sunken` track, raised in both modes. With focus,
/// ← and → pick the neighbor. `on_click` gets the index; the caller sets
/// [`Self::selected_index`].
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    segments: Vec<Segment>,
    selected: Option<usize>,
    on_click: Option<OnPick>,
    full_width: bool,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: None,
            on_click: None,
            full_width: false,
        }
    }

    pub fn child(mut self, segment: Segment) -> Self {
        self.segments.push(segment);
        self
    }

    pub fn children(mut self, segments: impl IntoIterator<Item = Segment>) -> Self {
        self.segments.extend(segments);
        self
    }

    pub fn selected_index(mut self, ix: usize) -> Self {
        self.selected = Some(ix);
        self
    }

    pub fn on_click(mut self, pick: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(pick));
        self
    }

    /// As wide as its parent, the segments sharing it equally.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
}

/// The next enabled segment from `from` in `step`'s direction, if any.
fn step_segment(enabled: &[bool], from: Option<usize>, step: isize) -> Option<usize> {
    let n = enabled.len() as isize;
    let mut ix = match from {
        Some(ix) => ix as isize,
        None if step > 0 => -1,
        None => n,
    };
    loop {
        ix += step;
        if !(0..n).contains(&ix) {
            return None;
        }
        if enabled[ix as usize] {
            return Some(ix as usize);
        }
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle().tab_stop(true))
            .read(cx)
            .clone();
        let theme = cx.theme();
        let (track, raised) = (theme.tab_bar_segmented, theme.tab_active);
        let (text, quiet, ring) = (theme.tab_active_foreground, theme.tab_foreground, theme.ring);
        let (radius, inner) = (theme.radius_lg, theme.radius);
        let enabled: Vec<bool> = self.segments.iter().map(|s| !s.disabled).collect();
        let (selected, on_click) = (self.selected, self.on_click);
        let keys = on_click.clone();
        let id = self.id.clone();
        let ring = vec![BoxShadow {
            color: ring,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
            inset: false,
        }];
        h_flex()
            .id(self.id.clone())
            .track_focus(&focus)
            .focus_visible(move |s| s.shadow(ring))
            .when(self.full_width, |el| el.w_full())
            .h(px(32.0))
            .p(px(4.0))
            .gap_0p5()
            .rounded(radius)
            .bg(track)
            .on_key_down(move |event, window, cx| {
                let step = match event.keystroke.key.as_str() {
                    "left" => -1,
                    "right" => 1,
                    _ => return,
                };
                cx.stop_propagation();
                if let (Some(pick), Some(to)) = (&keys, step_segment(&enabled, selected, step)) {
                    pick(&to, window, cx);
                }
            })
            .children(self.segments.into_iter().enumerate().map(|(ix, segment)| {
                let is_selected = selected == Some(ix);
                let pick = on_click.clone().filter(|_| !segment.disabled && !is_selected);
                let selector = format!("{id}-{ix}");
                h_flex()
                    .id(ix)
                    .debug_selector(move || selector.clone())
                    .when(self.full_width, |el| el.flex_1())
                    .h_full()
                    .px_3()
                    .justify_center()
                    .rounded(inner)
                    .text_sm()
                    .whitespace_nowrap()
                    .map(|el| match (is_selected, segment.disabled) {
                        (true, _) => el.bg(raised).shadow_sm().text_color(text),
                        (false, true) => el.text_color(quiet).opacity(0.5),
                        (false, false) => el.text_color(quiet).cursor_pointer(),
                    })
                    .when_some(segment.tooltip, |el, build| el.tooltip(move |w, cx| build(w, cx)))
                    .when_some(pick, |el, pick| {
                        el.on_click(move |_, window, cx| pick(&ix, window, cx))
                    })
                    .child(segment.label)
                    .children(segment.suffix)
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{Context, TestAppContext, VisualTestContext};

    use super::*;

    #[test]
    fn the_arrows_pass_over_disabled_segments() {
        let enabled = [true, false, true];
        assert_eq!(step_segment(&enabled, Some(0), 1), Some(2));
        assert_eq!(step_segment(&enabled, Some(2), -1), Some(0));
        assert_eq!(step_segment(&enabled, Some(2), 1), None, "no wrap");
        assert_eq!(step_segment(&enabled, None, 1), Some(0));
    }

    /// Three segments, the middle one disabled; picks land in `picked`.
    struct Three {
        selected: usize,
        picked: Rc<Cell<Option<usize>>>,
    }

    impl Render for Three {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let picked = self.picked.clone();
            let this = cx.weak_entity();
            div().size_full().child(
                Segmented::new("three")
                    .child(Segment::new("One"))
                    .child(Segment::new("Two").disabled(true))
                    .child(Segment::new("Three"))
                    .selected_index(self.selected)
                    .on_click(move |ix, _, cx| {
                        picked.set(Some(*ix));
                        let ix = *ix;
                        this.update(cx, |this, cx| {
                            this.selected = ix;
                            cx.notify();
                        })
                        .ok();
                    }),
            )
        }
    }

    /// A click picks a segment (not a disabled or selected one); focused, →
    /// picks the next enabled.
    #[gpui::test]
    fn a_segment_is_picked_by_a_click_and_the_arrows(cx: &mut TestAppContext) {
        let picked = Rc::new(Cell::new(None));
        let shared = picked.clone();
        let (window, view) = crate::test_support::open(cx, move |_, cx| {
            cx.new(|_| Three { selected: 0, picked: shared })
        });
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let center = |vcx: &mut VisualTestContext, ix: usize| {
            vcx.debug_bounds(format!("three-{ix}").leak()).expect("a segment").center()
        };
        let two = center(&mut vcx, 1);
        vcx.simulate_click(two, gpui::Modifiers::none());
        assert_eq!(picked.get(), None, "disabled");
        let three = center(&mut vcx, 2);
        vcx.simulate_click(three, gpui::Modifiers::none());
        assert_eq!(picked.get(), Some(2));
        assert_eq!(view.read_with(&vcx, |v, _| v.selected), 2);
        // The track takes focus from the click; ← passes over Two.
        vcx.simulate_keystrokes("left");
        assert_eq!(picked.get(), Some(0));
        assert_eq!(view.read_with(&vcx, |v, _| v.selected), 0);
    }
}
