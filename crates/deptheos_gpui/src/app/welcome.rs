//! An empty stack (first run, or New Stack): the viewer becomes a drop
//! target with Add Frames…, Add Folder…, the formats read, Try a sample
//! stack (`app/sample.rs`), three steps and the way to a batch. On a blank
//! Library the app's icon leads it.

use gpui::prelude::*;
use gpui::{App, Context, FontWeight, SharedString, Window, div, img, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use super::actions::{AddFolder, AddImages, KEY_CONTEXT, RunStack, SaveResult, bound_keys};
use super::sample::SampleStack;
use super::{AppView, Screen};
use crate::words::{DesktopWords, Wording as _, say};

/// The app's icon on a blank Library's welcome, in points.
pub(super) const WELCOME_ICON_SIZE: f32 = 96.0;

/// The three steps: each one's title and line, with the keys that stack
/// and export as this platform writes them.
pub(super) fn step_texts(w: &DesktopWords, stack: &str, export: &str) -> [(String, String); 3] {
    [
        (say!(w, "welcome-step-add"), say!(w, "welcome-step-add-text")),
        (say!(w, "welcome-step-stack"), say!(w, "welcome-step-stack-text", keys = stack)),
        (say!(w, "welcome-step-export"), say!(w, "welcome-step-export-text", keys = export)),
    ]
}

impl AppView {
    /// Whether the screen has nothing in it yet: a stack with no frames,
    /// or a Library with no stack.
    pub(super) fn is_empty(&self, cx: &App) -> bool {
        if self.is_processing(cx) {
            return false;
        }
        match self.screen {
            Screen::Stacking => {
                self.files.read(cx).rows().next().is_none()
                    && !self.has_result(cx)
                    && !self.is_retouching()
            }
            Screen::Library => self.library.read(cx).is_blank(cx),
        }
    }

    /// Under the buttons, after "No frames of your own yet?": Try a sample
    /// stack as a link, opening a menu for two samples, adding the one, or
    /// opening the website for none; inert while copying. A link, since two
    /// two-line buttons don't fit the smallest window under the icon.
    fn render_samples(&self, w: &DesktopWords, cx: &mut Context<Self>) -> gpui::Div {
        let muted = cx.theme().muted_foreground;
        let copying = self.sample_open.copying().is_some();
        let link = Button::new("welcome-sample")
            .label(match copying {
                true => say!(w, "welcome-sample-copying"),
                false => say!(w, "welcome-sample"),
            })
            .link()
            .small()
            .disabled(!self.can_try_sample(cx));
        let choices = self.sample_choices(w);
        let link = match choices.as_slice() {
            [] => link
                .on_click(cx.listener(|this, _, window, cx| this.try_sample(None, window, cx)))
                .into_any_element(),
            &[(one, ..)] => link
                .on_click(
                    cx.listener(move |this, _, window, cx| this.try_sample(Some(one), window, cx)),
                )
                .into_any_element(),
            _ => {
                let this = cx.weak_entity();
                link.dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (which, name, line) in choices.clone() {
                            menu = menu.item(
                                PopupMenuItem::element(move |_, cx| {
                                    v_flex()
                                        .debug_selector(move || format!("welcome-sample: {name}"))
                                        .py_0p5()
                                        .child(div().font_weight(FontWeight::MEDIUM).child(name))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(line.clone()),
                                        )
                                })
                                .on_click({
                                    let this = this.clone();
                                    move |_, window, cx| {
                                        this.update(cx, |this, cx| {
                                            this.try_sample(Some(which), window, cx)
                                        })
                                        .ok();
                                    }
                                }),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }
        };
        h_flex()
            .gap_1()
            .text_sm()
            .text_color(muted)
            .child(say!(w, "welcome-no-stack"))
            .child(div().debug_selector(|| "welcome-sample".into()).child(link))
    }

    /// The sample stacks the welcome offers, in order: each with its name
    /// and its line.
    pub(super) fn sample_choices(
        &self,
        w: &DesktopWords,
    ) -> Vec<(SampleStack, &'static str, String)> {
        self.samples
            .iter()
            .map(|sample| {
                let which = sample.stack();
                (which, which.name(), super::sample::line(which, w, sample.frame_count()))
            })
            .collect()
    }

    /// Over the viewer while the window is empty.
    pub(super) fn render_welcome(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let w = cx.wording().clone();
        let samples = self.render_samples(&w, cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let dragging = !self.file_drag.is_none();
        let border = if dragging { theme.primary } else { theme.border };
        // Wide enough that the formats line fits on one line at the
        // default text size.
        let drop = v_flex()
            .w(px(520.0))
            .max_w_full()
            .py_8()
            .px_6()
            .gap_4()
            .items_center()
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(border)
            .when(dragging, |el| el.bg(theme.primary.opacity(0.06)))
            .child(
                v_flex()
                    .items_center()
                    // A line that wraps stays centered under the heading.
                    .text_center()
                    .gap_1()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                        match self.screen {
                            Screen::Library => say!(w, "welcome-drop-library"),
                            Screen::Stacking => say!(w, "welcome-drop-stack"),
                        },
                    ))
                    .child(
                        div().text_sm().text_color(muted).child(say!(w, "welcome-drop-formats")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("welcome-add-images")
                            .icon(IconName::Plus)
                            .label(say!(w, "welcome-add-frames"))
                            .primary()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(AddImages), cx)
                            }),
                    )
                    .child(
                        Button::new("welcome-add-folder")
                            .label(say!(w, "welcome-add-folder"))
                            .outline()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(AddFolder), cx)
                            }),
                    ),
            )
            .child(samples);
        let mut steps = h_flex().w_full().max_w(px(640.0)).gap_4().items_start();
        let (stack_keys, export_keys) = (
            bound_keys(&RunStack, KEY_CONTEXT, window),
            bound_keys(&SaveResult, KEY_CONTEXT, window),
        );
        for (n, (title, text)) in step_texts(&w, &stack_keys, &export_keys).into_iter().enumerate()
        {
            steps = steps.child(
                h_flex()
                    .flex_1()
                    // Without a width of its own the text can't wrap, and
                    // the row runs on under the inspector.
                    .min_w_0()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .size(px(20.0))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(theme.secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from((n + 1).to_string())),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                            .child(div().text_xs().text_color(muted).child(text)),
                    ),
            );
        }
        let batch = h_flex()
            .debug_selector(|| "welcome-batch-line".into())
            .gap_1()
            .text_sm()
            .text_color(muted)
            .child(say!(w, "welcome-batch"))
            .child(
                Button::new("welcome-batch")
                    .label(match self.screen {
                        Screen::Library => say!(w, "welcome-batch-start-library"),
                        Screen::Stacking => say!(w, "welcome-batch-start-stack"),
                    })
                    .link()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.open_batch(window, cx))),
            );
        // The app's icon over the drop box, close to it, on the Library
        // only: an empty stack's welcome is about that stack.
        let icon = (self.screen == Screen::Library).then(|| {
            div()
                .debug_selector(|| "welcome-icon".into())
                .mb(px(-16.0))
                .child(img(super::update_check::app_icon()).size(px(WELCOME_ICON_SIZE)))
        });
        v_flex()
            .id("welcome")
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .gap_8()
            .px_6()
            .bg(theme.background)
            .children(icon)
            .child(drop)
            .child(steps)
            .child(batch)
    }
}
