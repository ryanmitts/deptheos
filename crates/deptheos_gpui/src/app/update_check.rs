//! Checking for a newer version, and the About window.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, ClipboardItem, Context, FontWeight, Image, ImageFormat, Window, div, img, px};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};

use super::AppView;
use crate::about;
use crate::notices::{Action, Kind, Subject};
use crate::updates::{self, Failure, Outcome};
use crate::words::say;

impl AppView {
    /// Ask GitHub for a newer release and offer its page. `manual` (Check
    /// for Updates…) also reports none or a failure; at startup it is quiet.
    pub(crate) fn check_for_updates(
        &mut self,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A manual check replaces one in flight (dropping its task), so it
        // reports even when the startup check would have stayed quiet.
        if self.update_check.is_some() && !manual {
            return;
        }
        let check = cx.background_spawn(async { updates::check() });
        self.update_check = Some(cx.spawn_in(window, async move |this, cx| {
            let outcome = check.await;
            this.update_in(cx, |this, window, cx| {
                this.update_check = None;
                let (kind, title, text, actions) = match outcome {
                    // It stays until closed, so Download can be reached.
                    Outcome::Newer(release) => {
                        let url = release.url;
                        let download = Action::new(say!(cx, "update-download"), move |_, cx| {
                            cx.open_url(&url)
                        });
                        let text = say!(
                            cx,
                            "update-available",
                            version = release.version.to_string(),
                            current = updates::current().to_string()
                        );
                        let title = Some(say!(cx, "update-available-title").into());
                        (Kind::Pinned, title, text, vec![download])
                    }
                    Outcome::UpToDate if manual => {
                        let text =
                            say!(cx, "update-up-to-date", version = updates::current().to_string());
                        (Kind::Done, None, text, Vec::new())
                    }
                    Outcome::Failed(failure) if manual => {
                        let text = match failure {
                            Failure::Unreachable => say!(cx, "update-failed-unreachable"),
                            Failure::Refused => say!(cx, "update-failed-refused"),
                            Failure::Unreadable => say!(cx, "update-failed-unreadable"),
                            Failure::NotAVersion(tag) => {
                                say!(cx, "update-failed-not-a-version", tag = tag)
                            }
                        };
                        (Kind::Error, None, text, Vec::new())
                    }
                    Outcome::Failed(failure) => {
                        tracing::warn!("update check failed: {failure:?}");
                        return;
                    }
                    Outcome::UpToDate => return,
                };
                // A second check's notice replaces the first one's.
                crate::notices::post_titled(
                    kind,
                    Subject::Update,
                    title,
                    text,
                    actions,
                    window,
                    cx,
                );
            })
            .ok();
        }));
    }

    pub(super) fn show_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        fn open(names: &'static [&'static str], cx: &mut App) {
            match about::locate(names) {
                about::Location::File(path) => cx.open_with_system(&path),
                about::Location::Url(url) => cx.open_url(&url),
            }
        }
        let title = say!(cx, "about-title");
        let name = say!(cx, "about-name");
        let version = say!(
            cx,
            "about-version",
            version = updates::current().to_string(),
            build = env!("DEPTHEOS_BUILD")
        );
        let tagline = say!(cx, "about-tagline");
        let license_text = say!(cx, "about-license-text");
        let license = say!(cx, "about-license");
        let notices = say!(cx, "about-notices");
        let (copy, copy_tip) =
            (say!(cx, "about-copy-version"), say!(cx, "about-copy-version-tooltip"));
        let (website, website_tip) = (say!(cx, "about-website"), say!(cx, "about-website-tooltip"));
        let gpu = self.sequences.read(cx).engine().gpu().adapter_info.name.clone();
        let copied = version_text(&deptheos_app::engine::logs::system::os_version(), &gpu);
        let icon = app_icon();
        let muted = cx.theme().muted_foreground;
        window.open_dialog(cx, move |dialog, _, _| {
            let copied = copied.clone();
            dialog
                .title(title.clone())
                .child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(img(icon.clone()).size(px(64.0)))
                        .child(
                            div().text_base().font_weight(FontWeight::SEMIBOLD).child(name.clone()),
                        )
                        .child(div().text_sm().text_color(muted).child(version.clone()))
                        .child(div().text_sm().child(tagline.clone()))
                        .child(
                            h_flex()
                                .pt_2()
                                .gap_2()
                                .child(
                                    Button::new("about-copy-version")
                                        .label(copy.clone())
                                        .small()
                                        .tooltip(copy_tip.clone())
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copied.clone(),
                                            ))
                                        }),
                                )
                                .child(
                                    Button::new("about-website")
                                        .label(website.clone())
                                        .small()
                                        .link()
                                        .tooltip(website_tip.clone())
                                        .on_click(|_, _, cx| cx.open_url(&updates::site_page("/"))),
                                ),
                        ),
                )
                .child(div().pt_2().text_xs().text_color(muted).child(license_text.clone()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("about-license")
                                .label(license.clone())
                                .small()
                                .on_click(|_, _, cx| open(about::LICENSE, cx)),
                        )
                        .child(
                            Button::new("about-notices")
                                .label(notices.clone())
                                .small()
                                .on_click(|_, _, cx| open(about::NOTICES, cx)),
                        ),
                )
                .button_props(DialogButtonProps::default().show_cancel(false))
        });
    }
}

/// The app's icon, as the Linux packages carry it: 256 px, so it stays
/// crisp at the welcome's 96 pt and About's 64 pt on a 2× display.
const APP_ICON: &[u8] = include_bytes!(
    "../../../../packaging/linux/icons/hicolor/256x256/apps/com.deptheos.Deptheos.png"
);

/// [`APP_ICON`] as an image to draw, made once: About and the welcome
/// share it.
pub(super) fn app_icon() -> Arc<Image> {
    static ICON: std::sync::LazyLock<Arc<Image>> = std::sync::LazyLock::new(|| {
        Arc::new(Image::from_bytes(ImageFormat::Png, APP_ICON.to_vec()))
    });
    ICON.clone()
}

/// What Copy Version copies: the version and build, the system and the GPU,
/// nothing else.
fn version_text(os: &str, gpu: &str) -> String {
    format!("Deptheos {} ({})\n{os}\nGPU: {gpu}", updates::current(), env!("DEPTHEOS_BUILD"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_version_says_the_version_build_system_and_gpu() {
        let text = version_text("macOS 15.2", "Apple M2 Max");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(
            lines[0],
            format!("Deptheos {} ({})", updates::current(), env!("DEPTHEOS_BUILD"))
        );
        assert_eq!(lines[1..], ["macOS 15.2", "GPU: Apple M2 Max"]);
    }

    #[test]
    fn the_icon_is_a_png() {
        assert_eq!(&APP_ICON[1..4], b"PNG");
        let size = image::load_from_memory(APP_ICON).map(|i| (i.width(), i.height())).unwrap();
        assert_eq!(size, (256, 256));
    }
}
