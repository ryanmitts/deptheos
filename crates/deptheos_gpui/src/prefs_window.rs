//! The Settings window: a pane per part (General, Export, Library, RAW,
//! Alignment, Advanced) picked from its top bar, each gpui-kit's settings
//! layout over [`crate::prefs`]. Titled after the pane, fixed size, closes
//! with ⌘W or Esc.
//!
//! Every field reads the global and writes through [`update_prefs`], so the
//! window holds no state of its own. Whether its paths exist is checked off
//! the UI thread ([`PathChecks`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use deptheos_app::{DeviceChoice, DeviceInfo, DeviceKind, GpuContext};
use gpui::prelude::*;
use gpui::{
    AnyWindowHandle, App, Axis, Bounds, Context, FocusHandle, Focusable, Global, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};

use crate::app::actions::{CloseWindow, SETTINGS_KEY_CONTEXT};
use crate::app::{diagnostics, library_delete};
use crate::file_dialog::{Picker, file_dialog};
use crate::look::Pickable as _;
use crate::prefs::{
    CONVERTER_JOBS, Keyed, PREFETCH_MB, Preferences, Prefs, device_from_key, device_key, prefs,
    update_prefs,
};
use crate::words::{DesktopWords, Wording as _, say};
use deptheos_app::library::history;
use deptheos_app::preferences::{Preference, grid_limits_help, grid_limits_label};

/// The open Preferences window, so the menu item brings it forward.
#[derive(Default)]
struct PreferencesWindow(Option<AnyWindowHandle>);

impl Global for PreferencesWindow {}

/// `handle` is the Preferences window (the app quits when only it is left).
pub fn is_preferences_window(handle: AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<PreferencesWindow>().and_then(|w| w.0) == Some(handle)
}

/// Bring the Preferences window forward, opening it if need be.
pub fn open_preferences(cx: &mut App) {
    if let Some(handle) = cx.try_global::<PreferencesWindow>().and_then(|w| w.0)
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(900.0), px(640.0)), cx);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some((PANES[0].name)(cx.wording()).into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        is_resizable: false,
        is_minimizable: false,
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let build = |window: &mut gpui::Window, cx: &mut gpui::App| {
        crate::color::tag_window(window, deptheos_app::engine::present::color_managed());
        crate::follow_system_appearance(window);
        let view = cx.new(PreferencesView::new);
        // Focused, so ⌘W and Esc reach it.
        window.focus(&view.focus_handle(cx), cx);
        view
    };
    match gpui_kit::open_window(options, cx, build) {
        Ok((handle, _)) => cx.set_global(PreferencesWindow(Some(handle))),
        Err(e) => tracing::error!("could not open the Settings window: {e}"),
    }
}

/// This machine's GPUs and the engine's device, for the Graphics setting.
#[derive(Default)]
struct Gpus {
    /// `None` until [`find_gpus`]' search is done.
    found: Option<Vec<DeviceInfo>>,
    /// `None` when the engine could not start.
    in_use: Option<DeviceInfo>,
    /// The choice that failed at launch, and why; the engine runs on Automatic.
    failed: Option<(DeviceChoice, String)>,
}

impl Global for Gpus {}

/// Note the engine's device and list the GPUs in the background (a few
/// tenths of a second), unless `found` is already a list.
pub fn find_gpus(
    found: Option<Vec<DeviceInfo>>,
    in_use: Option<DeviceInfo>,
    failed: Option<(DeviceChoice, String)>,
    cx: &mut App,
) {
    if let Some(found) = found {
        cx.set_global(Gpus { found: Some(found), in_use, failed });
        return;
    }
    cx.set_global(Gpus { found: None, in_use, failed });
    let search = crate::work::background("find-gpus", GpuContext::devices);
    cx.spawn(async move |cx| {
        let found = search.await.unwrap_or_else(|e| {
            tracing::warn!("the GPUs couldn't be listed: {e}");
            Vec::new()
        });
        cx.update(|cx| cx.global_mut::<Gpus>().found = Some(found));
    })
    .detach();
}

/// A pane: its key (naming the bar's selectors), name, icon and groups.
struct Pane {
    key: &'static str,
    name: fn(&DesktopWords) -> String,
    icon: IconName,
    groups: fn(&App) -> Vec<SettingGroup>,
}

/// The panes, in the bar's order.
const PANES: [Pane; 6] = [
    Pane {
        key: "General",
        name: |w| say!(w, "prefs-pane-general"),
        icon: IconName::Settings,
        groups: general_page,
    },
    Pane {
        key: "Export",
        name: |w| say!(w, "prefs-pane-export"),
        icon: IconName::Share,
        groups: export_page,
    },
    Pane {
        key: "Library",
        name: |w| say!(w, "prefs-pane-library"),
        icon: IconName::LayoutGrid,
        groups: library_page,
    },
    // Named by the format alone.
    Pane { key: "RAW", name: |_| "RAW".to_owned(), icon: IconName::Camera, groups: raw_page },
    Pane {
        key: "Alignment",
        name: |w| say!(w, "prefs-pane-alignment"),
        icon: IconName::Layers2,
        groups: alignment_page,
    },
    Pane {
        key: "Advanced",
        name: |w| say!(w, "prefs-pane-advanced"),
        icon: IconName::SlidersHorizontal,
        groups: advanced_page,
    },
];

pub struct PreferencesView {
    /// The pane shown, an index into [`PANES`].
    pane: usize,
    /// The window's title, once set to the pane's name.
    titled: Option<usize>,
    /// Whether the Library can be deleted, as last drawn, so the window
    /// redraws only when that changes.
    deletable: library_delete::Deletable,
    /// The file or folder dialog a Choose… opened, one at a time.
    picker: Picker,
    focus_handle: FocusHandle,
    _observe: Vec<gpui::Subscription>,
}

impl Focusable for PreferencesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PreferencesView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut observe = vec![
            cx.observe_global::<Prefs>(|_, cx| {
                check_paths(false, cx);
                cx.notify()
            }),
            cx.observe_global::<Gpus>(|_, cx| cx.notify()),
            cx.observe_global::<PathChecks>(|_, cx| cx.notify()),
        ];
        if let Some(main) = library_delete::main_view(cx) {
            observe.push(cx.observe(&main, |this, _, cx| {
                let now = library_delete::deletable(cx);
                if now != this.deletable {
                    this.deletable = now;
                    cx.notify();
                }
            }));
        }
        check_paths(true, cx);
        Self {
            pane: 0,
            titled: None,
            deletable: library_delete::deletable(cx),
            picker: Picker::default(),
            focus_handle: cx.focus_handle(),
            _observe: observe,
        }
    }

    /// The bar of panes along the top.
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut bar = h_flex()
            .w_full()
            .justify_center()
            .gap_1()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary);
        for (ix, &Pane { key, name, icon, .. }) in PANES.iter().enumerate() {
            let selected = ix == self.pane;
            let name = name(cx.wording());
            bar = bar.child(
                v_flex()
                    .id(("settings-pane", ix))
                    .debug_selector(move || format!("settings-pane-{key}"))
                    .w(px(84.0))
                    .py_1p5()
                    .gap_1()
                    .items_center()
                    .rounded(theme.radius)
                    .text_xs()
                    .pickable(selected, true, cx)
                    .when(!selected, |el| el.text_color(muted))
                    .child(Icon::new(icon).size(px(20.0)))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pane = ix;
                        cx.notify();
                    })),
            );
        }
        bar
    }
}

impl Render for PreferencesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Titled after the pane, as System Settings is.
        if self.titled != Some(self.pane) {
            window.set_window_title(&(PANES[self.pane].name)(cx.wording()));
            self.titled = Some(self.pane);
        }
        // The pane's name is the title and selected in the bar, so the header
        // holds only Reset All, floating top right beside the first heading
        // rather than taking a band.
        let header = gpui::StyleRefinement::default()
            .absolute()
            .top_0()
            .right_0()
            .pt_3p5()
            .pr_4()
            .pb_0()
            .pl_0()
            .border_0();
        let pages = PANES.iter().map(|pane| {
            SettingPage::new("").header_style(&header).resettable(true).groups((pane.groups)(cx))
        });
        // Each pane its own layout, sidebar folded: the bar picks the page.
        let settings = Settings::new(("settings", self.pane))
            .default_selected_index(SelectIndex { page_ix: self.pane, group_ix: None })
            .sidebar_width(px(0.0))
            .sidebar_size_range(px(0.0)..px(0.0))
            .pages(pages);
        // The main window's sizes: group headings as the inspector's.
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(SETTINGS_KEY_CONTEXT)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .child(self.render_bar(cx))
            .child(
                div().debug_selector(|| "settings-page".into()).flex_1().min_h_0().child(settings),
            )
    }
}

// ----------------------------------------------------------------------
// Pages
// ----------------------------------------------------------------------

fn general_page(cx: &App) -> Vec<SettingGroup> {
    let defaults = Preferences::default();
    vec![
        SettingGroup::new().title(say!(cx, "prefs-appearance")).item(dropdown(
            cx.wording(),
            say!(cx, "prefs-theme"),
            say!(cx, "prefs-theme-description"),
            |p| p.appearance,
            |p, v| p.appearance = v,
            defaults.appearance,
        )),
        SettingGroup::new().title(say!(cx, "prefs-updates")).item(switch(
            say!(cx, "prefs-check-for-updates"),
            say!(cx, "prefs-check-for-updates-description"),
            |p| p.check_for_updates,
            |p, v| p.check_for_updates = v,
            defaults.check_for_updates,
        )),
    ]
}

fn export_page(cx: &App) -> Vec<SettingGroup> {
    let defaults = Preferences::default();
    let pattern_field = SettingField::input(
        |cx| prefs(cx).export.file_name_pattern.clone().into(),
        |value: SharedString, cx| {
            update_prefs(cx, |p| p.export.file_name_pattern = value.to_string())
        },
    )
    .default_value(SharedString::from(defaults.export.file_name_pattern.clone()));
    vec![
        SettingGroup::new().title(say!(cx, "prefs-export-defaults")).items([
            dropdown(
                cx.wording(),
                say!(cx, "prefs-format"),
                say!(cx, "prefs-format-description"),
                |p| p.save_format,
                |p, v| p.save_format = v,
                defaults.save_format,
            ),
            dropdown(
                cx.wording(),
                say!(cx, "prefs-format-raw"),
                say!(cx, "prefs-format-raw-description"),
                |p| p.save_format_raw,
                |p, v| p.save_format_raw = v,
                defaults.save_format_raw,
            ),
            dropdown(
                cx.wording(),
                Preference::TiffCompression.label(crate::words::of(cx)),
                Preference::TiffCompression.help(crate::words::of(cx)),
                |p| p.export.tiff_compression,
                |p, v| p.export.tiff_compression = v,
                defaults.export.tiff_compression,
            ),
            shared_number(
                cx,
                Preference::JpegQuality,
                |p| f32::from(p.export.jpeg_quality),
                |p, v| p.export.jpeg_quality = v.round() as u8,
                f32::from(defaults.export.jpeg_quality),
            ),
        ]),
        SettingGroup::new().title(say!(cx, "prefs-folder-and-name")).items([
            dropdown(
                cx.wording(),
                say!(cx, "prefs-export-to"),
                say!(cx, "prefs-export-to-description"),
                |p| p.export_place,
                |p, v| p.export_place = v,
                defaults.export_place,
            ),
            SettingItem::render(|_, _, cx| name_pattern_preview(cx)),
            SettingItem::new(Preference::FileName.label(crate::words::of(cx)), pattern_field)
                .description(Preference::FileName.help(crate::words::of(cx))),
            switch(
                crate::form::reveal_when_done_label(cx),
                say!(cx, "prefs-reveal-export-description"),
                |p| p.reveal_export,
                |p, v| p.reveal_export = v,
                defaults.reveal_export,
            ),
        ]),
    ]
}

/// "For example: bee-4.tif", from the pattern typed.
fn name_pattern_preview(cx: &App) -> impl IntoElement + use<> {
    let p = prefs(cx);
    let extension = p.save_format.extension();
    let example =
        p.export.file_name_example(extension, &crate::export_dialog::today(), crate::words::of(cx));
    div().text_xs().text_color(cx.theme().muted_foreground).child(example)
}

fn library_page(cx: &App) -> Vec<SettingGroup> {
    vec![
        SettingGroup::new().title(say!(cx, "prefs-renders")).item(
            path_item(
                say!(cx, "prefs-library-folder"),
                SettingField::render(|_, _, cx| {
                    let p = prefs(cx);
                    let shown = p.history_root();
                    folder_row(
                        "history",
                        shown.as_deref(),
                        p.history_dir.is_some(),
                        say!(cx, "prefs-default"),
                        (refuse_library, |p, dir| p.history_dir = Some(dir)),
                        |p| p.history_dir = None,
                        cx,
                    )
                }),
                |p| p.history_dir.is_some(),
                |p| p.history_dir = None,
            )
            .description(say!(cx, "prefs-library-folder-description")),
        ),
        SettingGroup::new().item(delete_library_item(cx)),
    ]
}

/// Delete Library: what it does, and why it is unavailable when it is
/// (under it and as the tooltip).
fn delete_library_item(cx: &App) -> SettingItem {
    let what = say!(cx, "prefs-delete-library-description");
    let description = match library_delete::deletable(cx).reason(cx.wording()) {
        Some(why) => deptheos_app::words::sentences(crate::words::of(cx), [what, why]),
        None => what,
    };
    let field = SettingField::render(|_, _, cx| delete_library_button(cx));
    SettingItem::new(say!(cx, "prefs-delete-library"), field).description(description)
}

/// The red Delete Library… button, disabled while it can't be used.
fn delete_library_button(cx: &App) -> impl IntoElement + use<> {
    let why = library_delete::deletable(cx).reason(cx.wording());
    Button::new("delete-library")
        .label(say!(cx, "prefs-delete-library-button"))
        .small()
        .danger()
        .disabled(why.is_some())
        .when_some(why, |button, why| button.tooltip(why))
        .on_click(|_, window, cx| ask_delete_library(window, cx))
}

/// Ask in `window` whether to delete the Library, naming its folder, what
/// it holds and frees (none until all is measured); the main window deletes
/// it. Cancel is the default, so Return deletes nothing.
fn ask_delete_library(window: &mut Window, cx: &mut App) {
    let library_delete::Deletable::Ready { stacks, renders } = library_delete::deletable(cx) else {
        return;
    };
    let Some(view) = library_delete::main_view(cx) else { return };
    let renders_of = view.read(cx).renders.read(cx);
    let folder = renders_of.store().root().display().to_string();
    let size = renders_of.library_size_text(cx.wording());
    let w = crate::words::of(cx);
    let title = history::delete_library_title(w);
    let description =
        history::delete_library_message(w, stacks, renders, Some(&folder), size.as_deref());
    let delete = history::delete_library_button(w);
    let confirm = |_: &mut Window, cx: &mut App| library_delete::delete_library(cx);
    crate::form::ask_destructive(title, description, delete, confirm, window, cx);
}

fn raw_page(cx: &App) -> Vec<SettingGroup> {
    let defaults = Preferences::default();
    vec![
        SettingGroup::new().title(say!(cx, "prefs-decoding")).items([
            dropdown(
                cx.wording(),
                say!(cx, "prefs-default-decoder"),
                say!(cx, "prefs-default-decoder-description"),
                |p| p.raw_backend,
                |p, v| p.raw_backend = v,
                defaults.raw_backend,
            ),
            dropdown(
                cx.wording(),
                say!(cx, "prefs-camera-profile"),
                say!(cx, "prefs-camera-profile-description"),
                |p| p.camera_profile,
                |p, v| p.camera_profile = v,
                defaults.camera_profile,
            ),
        ]),
        SettingGroup::new()
            .title(say!(cx, "prefs-profile-folders"))
            .description(held_description(say!(cx, "prefs-profile-folders-description"), cx))
            .items([
                SettingItem::render(|_, _, cx| profile_folders(cx)).on_reset(
                    |cx| !prefs(cx).profile_dirs.is_empty(),
                    |_, cx| update_prefs(cx, |p| p.profile_dirs.clear()),
                ),
                SettingItem::render(|_, _, cx| adobe_folders(cx)),
            ]),
        // The product's name.
        held(SettingGroup::new().title("Adobe DNG Converter"), cx).items([
            path_item(
                say!(cx, "prefs-converter"),
                SettingField::render(|_, _, cx| converter_row(cx)),
                |p| p.converter_path.is_some(),
                |p| p.converter_path = None,
            )
            .description(say!(cx, "prefs-converter-description")),
            number(
                say!(cx, "prefs-converter-jobs"),
                say!(cx, "prefs-converter-jobs-description"),
                (CONVERTER_JOBS.0 as f32, CONVERTER_JOBS.1 as f32, 1.0),
                |p| p.converter_jobs as f32,
                |p, v| p.converter_jobs = v.round() as u32,
                defaults.converter_jobs as f32,
            ),
            switch(
                say!(cx, "prefs-converter-uncompressed"),
                say!(cx, "prefs-converter-uncompressed-description"),
                |p| p.converter_uncompressed,
                |p, v| p.converter_uncompressed = v,
                defaults.converter_uncompressed,
            ),
        ]),
    ]
}

/// `group`, saying during a run that a change applies when it ends
/// ([`crate::prefs::environment_held`]).
fn held(group: SettingGroup, cx: &App) -> SettingGroup {
    match crate::prefs::environment_held(cx) {
        true => group.description(say!(cx, "prefs-applies-when-run-ends")),
        false => group,
    }
}

/// `description`, plus during a run that a change applies when it ends.
fn held_description(description: String, cx: &App) -> String {
    match crate::prefs::environment_held(cx) {
        true => format!("{description} {}", say!(cx, "prefs-applies-when-run-ends")),
        false => description,
    }
}

fn alignment_page(cx: &App) -> Vec<SettingGroup> {
    let d = Preferences::default();
    vec![
        SettingGroup::new()
            .title(grid_limits_label(crate::words::of(cx)))
            .description(grid_limits_help(crate::words::of(cx)))
            .items([
                shared_number(
                    cx,
                    Preference::GridHorizontal,
                    |p| p.alignment.grid_horizontal_pct,
                    |p, v| p.alignment.grid_horizontal_pct = v,
                    d.alignment.grid_horizontal_pct,
                ),
                shared_number(
                    cx,
                    Preference::GridVertical,
                    |p| p.alignment.grid_vertical_pct,
                    |p, v| p.alignment.grid_vertical_pct = v,
                    d.alignment.grid_vertical_pct,
                ),
                shared_number(
                    cx,
                    Preference::GridRotation,
                    |p| p.alignment.grid_rotation_deg,
                    |p, v| p.alignment.grid_rotation_deg = v,
                    d.alignment.grid_rotation_deg,
                ),
                shared_number(
                    cx,
                    Preference::GridMagnification,
                    |p| p.alignment.grid_magnification_pct,
                    |p, v| p.alignment.grid_magnification_pct = v,
                    d.alignment.grid_magnification_pct,
                ),
                shared_switch(
                    cx,
                    Preference::ExposureCompensation,
                    |p| p.alignment.exposure_compensation,
                    |p, v| p.alignment.exposure_compensation = v,
                    d.alignment.exposure_compensation,
                ),
            ]),
    ]
}

fn advanced_page(cx: &App) -> Vec<SettingGroup> {
    let d = Preferences::default();
    vec![
        graphics_group(cx),
        SettingGroup::new().title(say!(cx, "prefs-memory")).item(number(
            say!(cx, "prefs-read-ahead"),
            say!(cx, "prefs-read-ahead-description"),
            (PREFETCH_MB.0 as f32, PREFETCH_MB.1 as f32, 128.0),
            |p| p.prefetch_mb as f32,
            |p, v| p.prefetch_mb = v.round() as u32,
            d.prefetch_mb as f32,
        )),
        diagnostics_group(cx),
    ]
}

/// Verbose logging: not a saved preference; it sets the log directly and is
/// off again next launch.
fn diagnostics_group(cx: &App) -> SettingGroup {
    let field = SettingField::switch(
        |_| diagnostics::is_verbose(),
        |on, cx| {
            diagnostics::set_verbose(on);
            // Nothing global changed for the window to redraw on.
            cx.refresh_windows();
        },
    )
    .default_value(false);
    SettingGroup::new().title(say!(cx, "prefs-diagnostics")).item(
        SettingItem::new(say!(cx, "prefs-verbose-logging"), field)
            .description(say!(cx, "prefs-verbose-logging-description")),
    )
}

/// The GPU choice: Automatic, each GPU found, Software; and what is in use.
fn graphics_group(cx: &App) -> SettingGroup {
    let gpus = cx.try_global::<Gpus>();
    let found = gpus.and_then(|g| g.found.as_deref()).unwrap_or_default();
    let mut options: Vec<(SharedString, SharedString)> =
        vec![(device_key(&DeviceChoice::Automatic).into(), say!(cx, "prefs-gpu-automatic").into())];
    options.extend(found.iter().map(|d| (d.name.clone().into(), device_label(d, cx).into())));
    // A chosen GPU that is not here (or not listed yet) stays shown.
    if let DeviceChoice::Named(name) = &prefs(cx).gpu
        && !found.iter().any(|d| &d.name == name)
    {
        let searched = gpus.is_some_and(|g| g.found.is_some());
        let label = if searched {
            say!(cx, "prefs-gpu-not-found", name = name.as_str())
        } else {
            name.clone()
        };
        options.push((name.clone().into(), label.into()));
    }
    options
        .push((device_key(&DeviceChoice::Software).into(), say!(cx, "prefs-gpu-software").into()));
    let field = SettingField::dropdown(
        options,
        |cx| device_key(&prefs(cx).gpu).to_owned().into(),
        |key, cx| update_prefs(cx, |p| p.gpu = device_from_key(&key)),
    )
    .default_value(device_key(&DeviceChoice::Automatic));
    SettingGroup::new().title(say!(cx, "prefs-graphics")).items([
        SettingItem::new(say!(cx, "prefs-gpu"), field)
            .description(say!(cx, "prefs-gpu-description")),
        SettingItem::render(|_, _, cx| gpu_status(cx)),
    ])
}

/// A device's name, with what kind it is where that helps choose.
fn device_label(device: &DeviceInfo, cx: &App) -> String {
    let name = device.name.as_str();
    match device.kind() {
        DeviceKind::Dedicated => say!(cx, "prefs-gpu-dedicated", name = name),
        DeviceKind::Integrated => say!(cx, "prefs-gpu-integrated", name = name),
        DeviceKind::Software => say!(cx, "prefs-gpu-cpu", name = name),
        DeviceKind::Other => device.name.clone(),
    }
}

/// The device in use, and whether the choice needs a restart to apply.
fn gpu_status(cx: &App) -> impl IntoElement + use<> {
    let gpus = cx.try_global::<Gpus>();
    let in_use = gpus.and_then(|g| g.in_use.as_ref());
    let found = gpus.and_then(|g| g.found.as_deref());
    let line = match in_use {
        Some(device) => say!(cx, "prefs-gpu-in-use", device = device_label(device, cx)),
        None => say!(cx, "prefs-gpu-none-started"),
    };
    let restart = || say!(cx, "prefs-gpu-restart");
    let forced = std::env::var_os("DEPTHEOS_SOFTWARE_GPU").is_some_and(|v| v != "0");
    let failed =
        gpus.and_then(|g| g.failed.as_ref()).filter(|(choice, _)| *choice == prefs(cx).gpu);
    let note = match (&prefs(cx).gpu, in_use, found) {
        _ if forced => Some(say!(cx, "prefs-gpu-forced")),
        _ if failed.is_some() => {
            failed.map(|(_, reason)| say!(cx, "prefs-gpu-failed", reason = reason.as_str()))
        }
        (_, None, _) | (_, _, None) => None,
        (DeviceChoice::Software, Some(device), _) => (!device.is_software()).then(restart),
        (DeviceChoice::Named(name), Some(device), Some(found)) => {
            if !found.iter().any(|d| &d.name == name) {
                Some(say!(cx, "prefs-gpu-missing", name = name.as_str()))
            } else {
                (&device.name != name).then(restart)
            }
        }
        // The fastest is the first found; with none, the CPU renderer.
        (DeviceChoice::Automatic, Some(device), Some(found)) => {
            let switching = match found.first() {
                Some(fastest) => device.name != fastest.name,
                None => !device.is_software(),
            };
            switching.then(restart)
        }
    };
    v_flex().gap_1().text_xs().child(line).when_some(note, |el, note| {
        el.child(div().text_color(cx.theme().muted_foreground).child(note))
    })
}

// ----------------------------------------------------------------------
// Field builders
// ----------------------------------------------------------------------

fn with_description(item: SettingItem, description: String) -> SettingItem {
    if description.is_empty() { item } else { item.description(description) }
}

fn dropdown<T: Keyed>(
    words: &DesktopWords,
    title: String,
    description: String,
    get: fn(&Preferences) -> T,
    set: fn(&mut Preferences, T),
    default: T,
) -> SettingItem {
    let options =
        T::ALL.iter().map(|v| (SharedString::from(v.key()), v.label(words).into())).collect();
    let field = SettingField::dropdown(
        options,
        move |cx| get(prefs(cx)).key().into(),
        move |key, cx| {
            if let Some(value) = T::from_key(&key) {
                update_prefs(cx, |p| set(p, value));
            }
        },
    )
    .default_value(default.key());
    with_description(SettingItem::new(title, field), description)
}

fn switch(
    title: String,
    description: String,
    get: fn(&Preferences) -> bool,
    set: fn(&mut Preferences, bool),
    default: bool,
) -> SettingItem {
    let field = SettingField::switch(
        move |cx| get(prefs(cx)),
        move |v, cx| update_prefs(cx, |p| set(p, v)),
    )
    .default_value(default);
    with_description(SettingItem::new(title, field), description)
}

/// [`switch`] for a preference shared with iOS, worded by the engine.
fn shared_switch(
    cx: &App,
    preference: Preference,
    get: fn(&Preferences) -> bool,
    set: fn(&mut Preferences, bool),
    default: bool,
) -> SettingItem {
    let words = crate::words::of(cx);
    switch(preference.label(words), preference.help(words), get, set, default)
}

/// [`number`] for a preference shared with iOS, with the engine's words and range.
fn shared_number(
    cx: &App,
    preference: Preference,
    get: fn(&Preferences) -> f32,
    set: fn(&mut Preferences, f32),
    default: f32,
) -> SettingItem {
    let words = crate::words::of(cx);
    let (lowest, max) = preference.range().expect("a number preference has a range");
    let range = (lowest.min, max, lowest.step);
    number(preference.label(words), preference.help(words), range, get, set, default)
}

/// A number field from `min` to `max` by `step`. It parses floats, so "nan"
/// arrives here and is left out.
fn number(
    title: String,
    description: String,
    (min, max, step): (f32, f32, f32),
    get: fn(&Preferences) -> f32,
    set: fn(&mut Preferences, f32),
    default: f32,
) -> SettingItem {
    let options =
        NumberFieldOptions { min: f64::from(min), max: f64::from(max), step: f64::from(step) };
    // Through f32's shortest decimal, so 0.1 shows as 0.1, not 0.100000001.
    let field = SettingField::number_input(
        options,
        move |cx| get(prefs(cx)).to_string().parse().unwrap_or_default(),
        move |v, cx| {
            if let Some(v) = in_range(v, min, max) {
                update_prefs(cx, |p| set(p, v));
            }
        },
    )
    .default_value(default.to_string().parse::<f64>().unwrap_or_default());
    with_description(SettingItem::new(title, field), description)
}

/// A field's value in `min..=max`; none for NaN (clamping leaves NaN).
fn in_range(v: f64, min: f32, max: f32) -> Option<f32> {
    v.is_finite().then(|| (v as f32).clamp(min, max))
}

// ----------------------------------------------------------------------
// Paths
// ----------------------------------------------------------------------

/// Ask for a file or folder (modal to `window`, opening in `start`), then
/// apply it; one dialog at a time.
fn pick(
    folder: bool,
    start: Option<&Path>,
    apply: impl FnOnce(&mut Preferences, PathBuf) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    pick_unless(folder, start, |_, _| None, apply, window, cx)
}

/// Why a folder chosen can't be used, said; none when it can.
type Refuse = fn(&Path, &App) -> Option<String>;

/// [`pick`], refusing with a message what `refuse` has a reason against.
/// The dialog is the view's [`Picker`], so a second Choose… does nothing.
fn pick_unless(
    folder: bool,
    start: Option<&Path>,
    refuse: Refuse,
    apply: impl FnOnce(&mut Preferences, PathBuf) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let root = window.root::<gpui_kit::base::Root>().flatten();
    let Some(view) =
        root.and_then(|r| r.read(cx).view().clone().downcast::<PreferencesView>().ok())
    else {
        return;
    };
    let dialog = file_dialog(window, start);
    view.update(cx, |view, cx| {
        view.picker.ask(
            |this: &mut PreferencesView| &mut this.picker,
            move || async move {
                let picked =
                    if folder { dialog.pick_folder().await } else { dialog.pick_file().await };
                picked.map(|handle| handle.path().to_path_buf())
            },
            move |_, picked, window, cx| {
                let Some(path) = picked else { return };
                match refuse(&path, cx) {
                    Some(why) => {
                        let (kind, subject) =
                            (crate::notices::Kind::Warning, crate::notices::Subject::Alone);
                        crate::notices::post(kind, subject, why, Vec::new(), window, cx)
                    }
                    None => update_prefs(cx, |p| apply(p, path)),
                }
            },
            window,
            cx,
        )
    });
}

/// Why `folder` can't be the Library's: one a sync service keeps.
pub(crate) fn refuse_library(folder: &Path, cx: &App) -> Option<String> {
    let service = deptheos_app::library::history::sync_service(folder)?;
    let refused = deptheos_app::StoreError::Synced { library: folder.to_path_buf(), service };
    let words = crate::words::of(cx);
    Some(deptheos_app::Explain::explain(&refused, words).text(words))
}

/// The paths Settings names, from the preferences: what it checks on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    /// Should be folders: the Library's and the profile folders.
    folders: Vec<PathBuf>,
    /// Should be a file: the converter.
    converter: Option<PathBuf>,
    /// Adobe's profile folders, shown only when they are there.
    adobe: Vec<PathBuf>,
}

impl Asked {
    fn of(cx: &App) -> Self {
        let p = prefs(cx);
        Self {
            folders: p.history_root().into_iter().chain(p.profile_dirs.iter().cloned()).collect(),
            converter: deptheos_app::converter_executable(),
            adobe: deptheos_app::adobe_profile_directories(),
        }
    }

    /// Look on disk, which a share gone away can take seconds to answer.
    fn check(&self) -> Found {
        let missing = self.folders.iter().filter(|p| !p.is_dir());
        let converter = self.converter.iter().filter(|p| !p.is_file());
        Found {
            missing: missing.chain(converter).cloned().collect(),
            adobe: self.adobe.iter().filter(|p| p.is_dir()).cloned().collect(),
        }
    }
}

/// What the disk said about an [`Asked`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Found {
    /// The folders and the file that aren't there.
    missing: HashSet<PathBuf>,
    /// Adobe's profile folders that are.
    adobe: Vec<PathBuf>,
}

/// Whether the paths Settings names exist, checked off the UI thread on
/// opening and on change, so drawing never waits on a disk.
///
/// Lifecycle: `Unchecked`, `Checking` or `Checked`, a global changed only by
/// `set_path_checks`, from [`check_paths`]. `Unchecked` or `Checked` →
/// `Checking` on opening or a path change (again while checking);
/// `Checking` → `Checked` when its check lands. The last findings show
/// meanwhile.
#[derive(Default)]
enum PathChecks {
    #[default]
    Unchecked,
    Checking {
        asked: Asked,
        shown: Option<Found>,
        _task: gpui::Task<()>,
    },
    Checked {
        asked: Asked,
        found: Found,
    },
}

impl Global for PathChecks {}

impl PathChecks {
    fn name(&self) -> &'static str {
        match self {
            PathChecks::Unchecked => "unchecked",
            PathChecks::Checking { .. } => "checking",
            PathChecks::Checked { .. } => "checked",
        }
    }

    fn asked(&self) -> Option<&Asked> {
        match self {
            PathChecks::Unchecked => None,
            PathChecks::Checking { asked, .. } | PathChecks::Checked { asked, .. } => Some(asked),
        }
    }

    fn found(&self) -> Option<&Found> {
        match self {
            PathChecks::Unchecked => None,
            PathChecks::Checking { shown, .. } => shown.as_ref(),
            PathChecks::Checked { found, .. } => Some(found),
        }
    }
}

fn set_path_checks(next: PathChecks, cx: &mut App) {
    let was = cx.try_global::<PathChecks>().map_or("unchecked", PathChecks::name);
    tracing::debug!("settings paths: {was} -> {}", next.name());
    cx.set_global(next);
}

/// Check the paths on a worker: again when `opened`, else only when they
/// differ from those checked or being checked.
fn check_paths(opened: bool, cx: &mut App) {
    let asked = Asked::of(cx);
    let checks = cx.try_global::<PathChecks>();
    if !opened && checks.and_then(PathChecks::asked) == Some(&asked) {
        return;
    }
    let shown = checks.and_then(PathChecks::found).cloned();
    let looked = crate::work::background("check-paths", {
        let asked = asked.clone();
        move || asked.check()
    });
    let task = cx.spawn({
        let asked = asked.clone();
        async move |cx| {
            let Ok(found) = looked.await else {
                tracing::warn!("the folders Settings names couldn't be checked");
                return;
            };
            cx.update(|cx| {
                let current = cx.try_global::<PathChecks>().and_then(PathChecks::asked);
                if matches!(cx.try_global::<PathChecks>(), Some(PathChecks::Checking { .. }))
                    && current == Some(&asked)
                {
                    set_path_checks(PathChecks::Checked { asked, found }, cx);
                }
            });
        }
    });
    set_path_checks(PathChecks::Checking { asked, shown, _task: task }, cx);
}

/// What the last check found, if one has.
fn found_paths(cx: &App) -> Option<&Found> {
    cx.try_global::<PathChecks>().and_then(PathChecks::found)
}

/// `path` was looked for and isn't there.
fn is_missing(path: &Path, cx: &App) -> bool {
    found_paths(cx).is_some_and(|f| f.missing.contains(path))
}

/// A path item: title and description above, the row below at full width
/// (beside the title column a path is cut off). A drawn field has no value
/// for the page's reset, so `is_set` and `clear` stand in.
fn path_item(
    title: String,
    field: SettingField<SharedString>,
    is_set: fn(&Preferences) -> bool,
    clear: fn(&mut Preferences),
) -> SettingItem {
    let field = field.on_reset(move |cx| is_set(prefs(cx)), move |_, cx| update_prefs(cx, clear));
    SettingItem::new(title, field).layout(Axis::Vertical)
}

/// A path and its buttons side by side while they fit, else the buttons on
/// the next line; the path wraps rather than being cut.
fn path_row(path: impl IntoElement, buttons: impl IntoElement) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().flex_1().min_w(px(200.0)).child(path))
        .child(h_flex().flex_none().gap_2().child(buttons))
}

/// A path in small text, dimmed, or in the danger color when `missing`.
fn path_text(
    path: Option<&Path>,
    placeholder: &str,
    missing: bool,
    cx: &App,
) -> impl IntoElement + use<> {
    let text: SharedString =
        path.map_or_else(|| placeholder.into(), |p| p.display().to_string().into());
    let color = if missing { cx.theme().danger } else { cx.theme().muted_foreground };
    div().w_full().text_xs().text_color(color).whitespace_normal().child(text)
}

/// A folder setting: the folder in effect and Choose / reset buttons.
fn folder_row(
    id: &'static str,
    shown: Option<&Path>,
    chosen: bool,
    placeholder: String,
    (refuse, set): (Refuse, fn(&mut Preferences, PathBuf)),
    clear: fn(&mut Preferences),
    cx: &App,
) -> impl IntoElement + use<> {
    let missing = shown.is_some_and(|p| is_missing(p, cx)) && chosen;
    let start = shown.map(Path::to_path_buf);
    let buttons = h_flex()
        .gap_2()
        .child(
            Button::new(SharedString::from(format!("{id}-choose")))
                .label(say!(cx, "button-choose"))
                .small()
                .on_click(move |_, window, cx| {
                    pick_unless(true, start.as_deref(), refuse, set, window, cx)
                }),
        )
        .when(chosen, |el| {
            el.child(
                Button::new(SharedString::from(format!("{id}-clear")))
                    .label(placeholder.clone())
                    .small()
                    .on_click(move |_, _, cx| update_prefs(cx, clear)),
            )
        });
    path_row(path_text(shown, &placeholder, missing, cx), buttons)
}

/// The converter in effect, whether it is there, and Choose / Default.
fn converter_row(cx: &App) -> impl IntoElement + use<> {
    let chosen = prefs(cx).converter_path.is_some();
    let exe = deptheos_app::converter_executable();
    // Until it has been looked for, neither found nor missing.
    let looked = found_paths(cx).is_some();
    let found = exe.as_deref().is_some_and(|p| !is_missing(p, cx));
    let status = match (&exe, looked, found) {
        (None, _, _) => Some(say!(cx, "prefs-converter-unavailable")),
        (Some(_), false, _) => None,
        (Some(_), true, true) => Some(say!(cx, "prefs-converter-found")),
        (Some(_), true, false) => Some(say!(cx, "prefs-converter-not-found")),
    };
    let buttons = h_flex()
        .gap_2()
        .child(Button::new("converter-choose").label(say!(cx, "button-choose")).small().on_click({
            let start = exe.as_deref().and_then(Path::parent).map(Path::to_path_buf);
            move |_, window, cx| {
                let set = |p: &mut Preferences, path| p.converter_path = Some(app_executable(path));
                pick(false, start.as_deref(), set, window, cx)
            }
        }))
        .when(exe.is_some() && !found, |el| {
            el.child(
                Button::new("converter-get")
                    .label(say!(cx, "prefs-converter-get"))
                    .small()
                    .on_click(|_, _, cx| cx.open_url(deptheos_app::CONVERTER_PAGE)),
            )
        })
        .when(chosen, |el| {
            el.child(
                Button::new("converter-default")
                    .label(say!(cx, "prefs-default"))
                    .small()
                    .on_click(|_, _, cx| update_prefs(cx, |p| p.converter_path = None)),
            )
        });
    v_flex()
        .w_full()
        .gap_1()
        .child(path_row(path_text(exe.as_deref(), "", !found, cx), buttons))
        .when_some(status, |el, status| el.child(div().text_xs().child(status)))
}

/// A macOS app bundle picked as the converter means its program
/// (`X.app/Contents/MacOS/X`).
fn app_executable(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) && path.is_dir() {
        let name = path.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
        let inner = path.join("Contents").join("MacOS").join(name);
        if inner.is_file() {
            return inner;
        }
    }
    path
}

/// The user's profile folders, each removable, and Add Folder….
fn profile_folders(cx: &App) -> impl IntoElement + use<> {
    let dirs = prefs(cx).profile_dirs.clone();
    v_flex()
        .w_full()
        .gap_1p5()
        .when(dirs.is_empty(), |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(say!(cx, "prefs-no-profile-folders")),
            )
        })
        .children(dirs.into_iter().enumerate().map(|(i, dir)| {
            let missing = is_missing(&dir, cx);
            let remove = Button::new(("remove-profile-dir", i))
                .label(say!(cx, "prefs-remove-profile-folder"))
                .small()
                .on_click(move |_, _, cx| {
                    update_prefs(cx, |p| {
                        if i < p.profile_dirs.len() {
                            p.profile_dirs.remove(i);
                        }
                    })
                });
            path_row(path_text(Some(&dir), "", missing, cx), remove)
        }))
        .child(
            h_flex().child(
                Button::new("add-profile-dir")
                    .label(say!(cx, "prefs-add-profile-folder"))
                    .small()
                    .on_click(|_, window, cx| {
                        pick(
                            true,
                            None,
                            |p, dir| {
                                if !p.profile_dirs.contains(&dir) {
                                    p.profile_dirs.push(dir);
                                }
                            },
                            window,
                            cx,
                        )
                    }),
            ),
        )
}

/// Adobe's own profile folders found on this machine.
fn adobe_folders(cx: &App) -> impl IntoElement + use<> {
    // Until they have been looked for, none are listed and none missing.
    let found: Option<Vec<PathBuf>> = found_paths(cx).map(|f| f.adobe.clone());
    v_flex()
        .gap_1()
        .child(div().text_xs().child(say!(cx, "prefs-adobe-profile-folders")))
        .when(found.as_ref().is_some_and(Vec::is_empty), |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(say!(cx, "prefs-no-adobe-profile-folders")),
            )
        })
        .children(found.iter().flatten().map(|p| path_text(Some(p.as_path()), "", false, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "nan" is no value; anything else lands in range.
    #[test]
    fn a_number_field_takes_only_numbers() {
        assert_eq!(in_range(f64::NAN, 0.0, 10.0), None);
        assert_eq!(in_range(f64::INFINITY, 0.0, 10.0), None);
        assert_eq!(in_range(42.0, 0.0, 10.0), Some(10.0));
        assert_eq!(in_range(2.5, 0.0, 10.0), Some(2.5));
    }

    /// A sync service's folder can't be the Library, and says so; others can.
    #[gpui::test]
    fn a_synced_folder_is_refused_for_the_library(cx: &mut gpui::TestAppContext) {
        crate::test_support::open(cx, |_, cx| cx.new(PreferencesView::new));
        let dir = tempfile::tempdir().unwrap();
        let dropbox = dir.path().join("Dropbox");
        std::fs::create_dir_all(dropbox.join("Deptheos")).unwrap();
        std::fs::write(dropbox.join(".dropbox"), "{}").unwrap();
        cx.update(|cx| {
            let why = refuse_library(&dropbox.join("Deptheos"), cx).expect("refused");
            assert!(why.contains("Dropbox") && why.contains("isn't synced"), "{why}");
            assert_eq!(refuse_library(dir.path(), cx), None);
        });
    }

    /// Every pane builds and draws (a malformed field panics only when drawn).
    #[gpui::test]
    fn window_draws(cx: &mut gpui::TestAppContext) {
        let (window, view) = crate::test_support::open(cx, |_, cx| cx.new(PreferencesView::new));
        cx.run_until_parked();
        // Every pane, as the bar picks it.
        for pane in 0..PANES.len() {
            view.update(cx, |v, cx| {
                v.pane = pane;
                cx.notify();
            });
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx)).unwrap();
        }
    }

    /// Reset All sits at the page's top right with a setting changed, and a
    /// click there resets the page.
    #[gpui::test]
    fn reset_all_is_at_the_top_right(cx: &mut gpui::TestAppContext) {
        let (window, _) = crate::test_support::open(cx, |_, cx| cx.new(PreferencesView::new));
        let default = Preferences::default().check_for_updates;
        cx.update(|cx| update_prefs(cx, |p| p.check_for_updates = !default));
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let page = vcx.debug_bounds("settings-page").expect("the page");
        let corner = gpui::point(page.right() - px(28.0), page.top() + px(26.0));
        vcx.simulate_click(corner, gpui::Modifiers::none());
        vcx.run_until_parked();
        assert_eq!(vcx.update(|_, cx| prefs(cx).check_for_updates), default, "reset");
    }

    /// Folders are checked on opening and on change, off the UI thread;
    /// drawing only reads the findings.
    #[gpui::test]
    fn missing_folders_are_looked_for_off_the_ui_thread(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("first"), dir.path().join("second"));
        cx.update(|cx| update_prefs(cx, |p| p.profile_dirs = vec![first.clone()]));
        crate::test_support::open(cx, |_, cx| cx.new(PreferencesView::new));
        let missing = |cx: &mut gpui::TestAppContext, path: &Path| {
            // Checked on the engine's pool.
            crate::test_support::wait_until(cx, "the check", |cx| {
                cx.read(|cx| {
                    matches!(cx.try_global::<PathChecks>(), Some(PathChecks::Checked { .. }))
                })
            });
            cx.read(|cx| {
                assert!(found_paths(cx).is_some(), "looked for");
                is_missing(path, cx)
            })
        };
        assert!(missing(cx, &first));

        // Made since: drawing doesn't look, but a change to the folders does.
        std::fs::create_dir(&first).unwrap();
        assert!(missing(cx, &first), "not looked for again until a change");
        cx.update(|cx| update_prefs(cx, |p| p.profile_dirs.push(second.clone())));
        assert!(!missing(cx, &first));
        assert!(missing(cx, &second));
    }

    #[test]
    fn app_bundle_resolves_to_its_program() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Adobe DNG Converter.app");
        let program = app.join("Contents/MacOS/Adobe DNG Converter");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"").unwrap();
        assert_eq!(app_executable(app), program);
        assert_eq!(app_executable(program.clone()), program);
    }
}
