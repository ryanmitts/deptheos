//! The window's actions, the keys bound to them and the menu bar.
//!
//! "secondary" is ⌘ on macOS and Ctrl elsewhere (`cmd` would be the
//! Windows key there).

use gpui::{Action, KeyBinding, Menu, MenuItem};

use crate::viewer::Look;
use crate::words::{DesktopWords, say};

/// The app view's key context outside paint mode; tooltips look shortcuts
/// up in it.
pub const KEY_CONTEXT: &str = "Deptheos";
/// The key context in paint mode; it keeps `Deptheos`, so view keys apply
/// where no paint binding overrides them.
pub const PAINT_KEY_CONTEXT: &str = "Deptheos paint";
/// The Library's key context: none of Stacking's keys apply.
pub const LIBRARY_KEY_CONTEXT: &str = "Library";
/// The root's key context while a stack runs: ⌘. cancels it.
pub const STACKING_KEY_CONTEXT: &str = "Deptheos stacking";
/// The Library's key context while a run goes: ⌘. cancels it.
pub const LIBRARY_RUNNING_KEY_CONTEXT: &str = "Library stacking";
/// The Settings window's key context: Esc closes it.
pub const SETTINGS_KEY_CONTEXT: &str = "Settings";

gpui::actions!(
    deptheos,
    [
        AddImages,
        NewStack,
        LibrarySelectAll,
        LibraryDeselect,
        LibraryDelete,
        LibraryFindStacks,
        LibraryFocusGrid,
        LibraryPreview,
        ClearFrames,
        OpenBatch,
        SaveResult,
        ExportAgain,
        ExportGrowSequence,
        RunStack,
        ToggleExcluded,
        ExcludeSelected,
        ZoomIn,
        ZoomOut,
        ZoomToFit,
        ZoomTo100,
        ToggleFit100,
        ViewResult,
        ToggleMap,
        ViewCompare,
        ViewSideBySide,
        ViewStacked,
        Flicker,
        SwapSides,
        StopCompare,
        ToggleContribution,
        About,
        CheckForUpdates,
        InstallCommandLineTool,
        OpenPreferences,
        Quit,
        TogglePaint,
        ToggleAdjust,
        TogglePick,
        LeavePaint,
        Undo,
        Redo,
        Eraser,
        BrushTool,
        SwapSource,
        NextFrame,
        PreviousFrame,
        BrushSmaller,
        BrushLarger,
        BrushSofter,
        BrushHarder,
        ToggleColorManagement,
        ToggleSidebar,
        ToggleInspector,
        ToggleFocusMode,
        RevertWorking,
        ShowStacking,
        ShowLibrary,
        OpenSelected,
        CancelStack,
        PeekSource,
        AddFolder,
        OpenWindowMenu,
        ShowFramesInFinder,
        CloseWindow,
        SelectAllFrames,
        RenameRender,
        RenameStack,
        IncludeOnlySelected,
        IncludeAllFrames,
        RenderInfo,
        ToggleFullScreen,
        MinimizeWindow,
        ZoomWindow,
        BringAllToFront,
        HideApp,
        HideOthers,
        ShowAll,
        OpenHelp,
        ShowShortcuts,
        ChoosingAMethod,
        ReportProblem,
        ReleaseNotes,
        ShowLogFolder,
        SaveDiagnostics,
        RunBenchmark
    ]
);

/// Enter Full Screen: ⌃⌘F on macOS, F11 elsewhere.
const FULL_SCREEN_KEY: &str = if cfg!(target_os = "macos") { "ctrl-cmd-f" } else { "f11" };

/// `keys` (macOS symbols, "⇧⌘E") as this platform writes them
/// ("Ctrl+Shift+E"); ⌃⌘ is Ctrl+Alt there, as for the panel keys.
pub(crate) fn key_text(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return keys.to_string();
    }
    // Ctrl first, as Windows writes them.
    keys.replace("⌃⌘", "Ctrl+Alt+")
        .replace("⇧⌘", "Ctrl+Shift+")
        .replace("⌥⌘", "Ctrl+Alt+")
        .replace('⌘', "Ctrl+")
        .replace('⇧', "Shift+")
        .replace('⌥', "Alt+")
        .replace('⌃', "Ctrl+")
        .replace('↩', "Enter")
        .replace('⌫', "Backspace")
        .replace('⌦', "Delete")
        .replace("Ctrl+-click", "Ctrl-click")
        .replace("Alt+-click", "Alt-click")
        .replace("Ctrl+-scroll", "Ctrl-scroll")
}

/// The keys bound to `action` in `context`, as this platform writes them
/// ("⌘⏎", "Ctrl+Enter"); empty when unbound.
pub(crate) fn bound_keys(action: &dyn Action, context: &str, window: &gpui::Window) -> String {
    use gpui::AsKeystroke as _;
    let Ok(context) = gpui::KeyContext::parse(context) else { return String::new() };
    window
        .highest_precedence_binding_for_action_in_context(action, context)
        .and_then(|binding| {
            binding
                .keystrokes()
                .first()
                .map(|k| gpui_kit::component::kbd::Kbd::format(k.as_keystroke()))
        })
        .unwrap_or_default()
}

/// The keys that show or hide the sidebar and the inspector.
pub(super) const PANEL_KEYS: [&str; 2] = if cfg!(target_os = "macos") {
    ["ctrl-cmd-s", "ctrl-cmd-i"]
} else {
    ["ctrl-alt-s", "ctrl-alt-i"]
};

/// Try a Sample Stack: the sample to open; `None` (a build without
/// samples) opens the website's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = deptheos, no_json)]
pub struct TrySample(pub Option<super::sample::SampleStack>);

/// Paint mode: set the brush opacity, in tenths (1–10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = deptheos, no_json)]
pub struct SetOpacity(pub u8);

/// Retouch's layouts, from the view bar and menu bar, bound to no key: in
/// Retouch 1–0 set opacity and \\ shows the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = deptheos, no_json)]
pub struct RetouchLayout(pub crate::viewer::Look);

/// The Library's arrows: a stack along the grid (`Previous`, `Next`) or a
/// row up or down; ⇧ extends from the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = deptheos, no_json)]
pub struct LibraryMove {
    pub step: crate::renders::Step,
    pub extend: bool,
}

/// The window's key bindings. ⌘O add, ⌘E export (⌘⇧E again), ⌘⇧B Batch…,
/// ⌘I Render Info, ⌘Q quit, ⌘, preferences, ⌘=/− zoom, ⌘0 fit, ⌘1 Actual
/// Size, ⌥⌘1/⌥⌘2 Stacking/Library, ⌘↩ stack; X excludes or includes. In the
/// viewer: 1–4 Result, Split, Side by Side, Top/Bottom; \\ flickers; S swaps;
/// Esc stops comparing; M map; +/− zoom, 0 and F fit; space fit/100%; C
/// Contribution (outside Retouch); P paint; K star. Paint mode overrides
/// some: 1–0 opacity, [ ] size, Shift-[ ] hardness, E/B/X source, ←/→ (or
/// PgUp/PgDn) frames, \\ held shows the source, ⌘Z undo, ⌘⇧Z (or Ctrl-Y)
/// redo, Esc leaves.
pub fn key_bindings() -> Vec<KeyBinding> {
    const CTX: Option<&str> = Some(KEY_CONTEXT);
    // Unmodified keys skip a focused input (the zoom field): otherwise a
    // binding up the focus path wins over the typed text.
    const KEYS: Option<&str> = Some("Deptheos && !Input");
    const PAINT: Option<&str> = Some("paint && !Input");
    const PAINT_CMD: Option<&str> = Some("paint");
    let mut bindings = vec![
        KeyBinding::new("secondary-n", NewStack, CTX),
        KeyBinding::new("secondary-n", NewStack, Some(LIBRARY_KEY_CONTEXT)),
        KeyBinding::new("secondary-o", AddImages, CTX),
        KeyBinding::new("secondary-shift-o", AddFolder, CTX),
        // After Stacking's so the menu shows that one; "Library" matches
        // during a run too, where the handlers refuse.
        KeyBinding::new("secondary-o", AddImages, Some(LIBRARY_KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-o", AddFolder, Some(LIBRARY_KEY_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAllFrames, KEYS),
        KeyBinding::new("secondary-w", CloseWindow, None),
        KeyBinding::new("escape", CloseWindow, Some("Settings && !Input")),
        KeyBinding::new(FULL_SCREEN_KEY, ToggleFullScreen, None),
        KeyBinding::new("secondary-/", ShowShortcuts, None),
        KeyBinding::new("secondary-e", SaveResult, CTX),
        KeyBinding::new("secondary-i", RenderInfo, CTX),
        KeyBinding::new("secondary-shift-e", ExportAgain, CTX),
        KeyBinding::new("secondary-shift-b", OpenBatch, CTX),
        KeyBinding::new("secondary-shift-b", OpenBatch, Some(LIBRARY_KEY_CONTEXT)),
        KeyBinding::new("secondary-alt-1", ShowStacking, None),
        KeyBinding::new("secondary-alt-2", ShowLibrary, None),
        KeyBinding::new("enter", OpenSelected, Some("Library && !Input")),
        KeyBinding::new("secondary-a", LibrarySelectAll, Some("Library && !Input")),
        KeyBinding::new("escape", LibraryDeselect, Some("Library && !Input")),
        KeyBinding::new("space", LibraryPreview, Some("Library && !Input")),
        KeyBinding::new("secondary-backspace", LibraryDelete, Some("Library && !Input")),
        KeyBinding::new("delete", LibraryDelete, Some("Library && !Input")),
        KeyBinding::new("secondary-f", LibraryFindStacks, Some(LIBRARY_KEY_CONTEXT)),
        // Esc in the search field gives the keys back to the grid.
        KeyBinding::new("escape", LibraryFocusGrid, Some("Library > Input")),
        KeyBinding::new("secondary-enter", RunStack, CTX),
        // The file list binds secondary-Enter itself; in the window it stacks.
        KeyBinding::new("secondary-enter", RunStack, Some("Deptheos > List")),
        KeyBinding::new("x", ToggleExcluded, KEYS),
        // In the frame list only: they exclude, never remove.
        KeyBinding::new("backspace", ExcludeSelected, Some("Deptheos > List")),
        KeyBinding::new("delete", ExcludeSelected, Some("Deptheos > List")),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenPreferences, None),
        // The ⌘ forms first: the macOS menu shows an action's first binding.
        KeyBinding::new("secondary-=", ZoomIn, CTX),
        KeyBinding::new("secondary--", ZoomOut, CTX),
        KeyBinding::new("secondary-0", ZoomToFit, CTX),
        KeyBinding::new("secondary-1", ZoomTo100, CTX),
        KeyBinding::new("=", ZoomIn, KEYS),
        KeyBinding::new("+", ZoomIn, KEYS),
        KeyBinding::new("-", ZoomOut, KEYS),
        KeyBinding::new("0", ZoomToFit, KEYS),
        KeyBinding::new("f", ZoomToFit, KEYS),
        KeyBinding::new("space", ToggleFit100, KEYS),
        KeyBinding::new("1", ViewResult, KEYS),
        KeyBinding::new("2", ViewCompare, KEYS),
        KeyBinding::new("3", ViewSideBySide, KEYS),
        KeyBinding::new("4", ViewStacked, KEYS),
        KeyBinding::new("\\", Flicker, KEYS),
        KeyBinding::new("s", SwapSides, KEYS),
        KeyBinding::new("escape", StopCompare, KEYS),
        // ⌘. (Ctrl+. elsewhere), as in Mac apps: Esc leaves Compare and
        // clears the Library's selection, so it never cancels a run.
        KeyBinding::new("secondary-.", CancelStack, Some("stacking")),
        KeyBinding::new("m", ToggleMap, KEYS),
        KeyBinding::new("c", ToggleContribution, KEYS),
        KeyBinding::new("p", TogglePaint, KEYS),
        KeyBinding::new("k", TogglePick, KEYS),
        // Not in the menu: for checking color management by eye.
        KeyBinding::new("secondary-alt-shift-c", ToggleColorManagement, CTX),
        // ⌃⌘S and ⌃⌘I on macOS, Ctrl-Alt elsewhere.
        KeyBinding::new(PANEL_KEYS[0], ToggleSidebar, CTX),
        KeyBinding::new(PANEL_KEYS[1], ToggleInspector, CTX),
        // The Library's root context isn't `Deptheos`; its inspector has the key too.
        KeyBinding::new(PANEL_KEYS[1], ToggleInspector, Some(LIBRARY_KEY_CONTEXT)),
        KeyBinding::new("tab", ToggleFocusMode, KEYS),
        // The next or previous compare frame. Arrows first: the macOS menu
        // shows an action's first binding.
        KeyBinding::new("right", NextFrame, KEYS),
        KeyBinding::new("left", PreviousFrame, KEYS),
        KeyBinding::new("pagedown", NextFrame, KEYS),
        KeyBinding::new("pageup", PreviousFrame, KEYS),
        // Paint mode: later bindings win, so these override the view keys.
        KeyBinding::new("escape", LeavePaint, PAINT),
        // Held: the source in place of the painted result.
        KeyBinding::new("\\", PeekSource, PAINT),
        KeyBinding::new("secondary-z", Undo, PAINT_CMD),
        KeyBinding::new("secondary-shift-z", Redo, PAINT_CMD),
        KeyBinding::new("e", Eraser, PAINT),
        KeyBinding::new("b", BrushTool, PAINT),
        KeyBinding::new("x", SwapSource, PAINT),
        KeyBinding::new("right", NextFrame, PAINT),
        KeyBinding::new("left", PreviousFrame, PAINT),
        KeyBinding::new("pagedown", NextFrame, PAINT),
        KeyBinding::new("pageup", PreviousFrame, PAINT),
        KeyBinding::new("[", BrushSmaller, PAINT),
        KeyBinding::new("]", BrushLarger, PAINT),
        KeyBinding::new("shift-[", BrushSofter, PAINT),
        KeyBinding::new("shift-]", BrushHarder, PAINT),
        KeyBinding::new("{", BrushSofter, PAINT),
        KeyBinding::new("}", BrushHarder, PAINT),
    ];
    // Ctrl-Y is redo on Windows and Linux; ⌘Y is something else on macOS.
    if cfg!(target_os = "macos") {
        // The macOS window and app keys; Windows has its own.
        bindings.extend([
            KeyBinding::new("cmd-m", MinimizeWindow, None),
            KeyBinding::new("cmd-h", HideApp, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-y", Redo, PAINT_CMD),
            // F10 opens the ≡ menu, as it does a menu bar.
            KeyBinding::new("f10", OpenWindowMenu, None),
        ]);
    }
    // The Library's arrows, in its context only (Stacking's step frames).
    {
        use crate::renders::Step;
        const LIBRARY_KEYS: Option<&str> = Some("Library && !Input");
        for (key, step) in [
            ("left", Step::Previous),
            ("right", Step::Next),
            ("up", Step::Up),
            ("down", Step::Down),
        ] {
            bindings.extend([
                KeyBinding::new(key, LibraryMove { step, extend: false }, LIBRARY_KEYS),
                KeyBinding::new(
                    &format!("shift-{key}"),
                    LibraryMove { step, extend: true },
                    LIBRARY_KEYS,
                ),
            ]);
        }
    }
    let digits = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
    bindings.extend(
        (1..=10).zip(digits).map(|(tenths, key)| KeyBinding::new(key, SetOpacity(tenths), PAINT)),
    );
    bindings
}

/// The Stack menu's Revert item, by kind rather than words.
///
/// Lifecycle: `None` (no render shown) or `Revert` to #n, enabled or not,
/// made each frame from `Showing` by `menu_state_in` (`app/menu.rs`) and
/// held in [`MenuState`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RevertItem {
    /// No render to go by: "Revert", disabled.
    #[default]
    None,
    /// "Revert to #3" (`Showing::revert`), asking first if unstacked changes
    /// would go; disabled when it would change nothing.
    Revert { number: u32, enabled: bool },
}

/// What the Stack menu's Cancel item names: the run going on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelItem {
    Stack,
    Batch,
    Sequence,
}

/// What the menu bar shows that depends on the window: flipping names,
/// checkmarks, and items that don't apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuState {
    /// What Undo and Redo would take back ("Paint"), while retouching.
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_result: bool,
    /// Stacking shows an image, so there is something to zoom.
    pub has_image: bool,
    /// Stack would start a stack now, as the toolbar's Stack button would.
    pub can_stack: bool,
    /// A run goes: New Stack, New Batch…, Add Frames…, Add Folder… and
    /// Export Grow Sequence… can't start one.
    pub running: bool,
    /// What Cancel (⌘.) would cancel; none while idle or already canceling.
    pub cancel: Option<CancelItem>,
    /// Export… and Export Again would export the result on screen.
    pub can_export: bool,
    /// Retouch would open paint mode, or paint mode is open to close.
    pub can_retouch: bool,
    /// A result is beside a frame or render, so there are sides to swap.
    pub comparing: bool,
    /// The layout: 0 Result, 1 Split, 2 Side by Side, 3 Top and Bottom,
    /// 4 Flicker; `None` while the map is shown.
    pub layout: Option<usize>,
    /// The depth map shown: `None` when there is none to show.
    pub map: Option<bool>,
    /// The contribution overlay: `None` when it can't be shown.
    pub contribution: Option<bool>,
    pub library: bool,
    pub sidebar: bool,
    /// The inspector is shown; `None` when it can't toggle (a blank Library).
    pub inspector: Option<bool>,
    pub focus: bool,
    /// The Stack menu's Revert item, for the render shown.
    pub revert: RevertItem,
    /// A render is shown on Stacking, so Render Info has one to show.
    pub info: bool,
    /// The render shown is starred; `None` when none is.
    pub picked: Option<bool>,
    /// X excludes (else includes) the selected frames.
    pub excludes: bool,
    /// No frame is excluded.
    pub all_included: bool,
    pub frames: bool,
    /// The stack on screen has a render, so a name to change.
    pub stacked: bool,
    pub painting: bool,
    /// This launch writes a log file, so Show Log Folder has one to show.
    pub log_folder: bool,
    /// Try a Sample Stack would act: nothing runs, no sample being tried
    /// (always, when it opens the website).
    pub can_try_sample: bool,
    /// The build's samples: a submenu for two or more, one item for one,
    /// the website's with none.
    pub sample_stacks: Vec<super::sample::SampleStack>,
    /// The window fills the screen, so the item leaves full screen.
    pub fullscreen: bool,
}

/// The menu bar's menus, as [`bar_menus`] makes them.
pub(crate) fn menus(state: &MenuState, words: &DesktopWords) -> Vec<Menu> {
    bar_menus(state, words).into_iter().map(|(_, menu)| menu).collect()
}

/// Which menu one is, so the ≡ menu arranges them by kind, not words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarMenu {
    App,
    File,
    Edit,
    View,
    Stack,
    Window,
    Help,
}

/// The menu bar (macOS only; elsewhere the toolbar's menu and keys reach
/// the rest), each menu with its kind. Items dim when they can't apply.
pub(crate) fn bar_menus(state: &MenuState, words: &DesktopWords) -> Vec<(BarMenu, Menu)> {
    let w = words;
    let s = state;
    let undo = match s.undo.as_deref() {
        Some(step) => say!(w, "menu-undo-step", step = step),
        None => say!(w, "menu-undo"),
    };
    let redo = match s.redo.as_deref() {
        Some(step) => say!(w, "menu-redo-step", step = step),
        None => say!(w, "menu-redo"),
    };
    // Show or hide, by whether it is shown.
    let show_hide = |shown: bool, hide: String, show: String| if shown { hide } else { show };
    // In Retouch the layout keys do other things, so its items show none,
    // and there is no Flicker.
    let layout = |name: String, ix: usize, look: Look, action: Box<dyn Action>| MenuItem::Action {
        name: name.into(),
        action: if s.painting { Box::new(RetouchLayout(look)) } else { action },
        os_action: None,
        checked: s.layout == Some(ix),
        disabled: !s.has_result || (s.painting && look == Look::Flicker),
    };
    let revert_item = match s.revert {
        RevertItem::None => MenuItem::action(say!(w, "menu-revert"), RevertWorking).disabled(true),
        RevertItem::Revert { number, enabled } => {
            MenuItem::action(say!(w, "menu-revert-to", number = number), RevertWorking)
                .disabled(!enabled)
        }
    };
    vec![
        (
            BarMenu::App,
            Menu::new(say!(w, "menu-app")).items([
                MenuItem::action(say!(w, "menu-about"), About),
                MenuItem::action(say!(w, "menu-check-for-updates"), CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-settings"), OpenPreferences),
                MenuItem::action(say!(w, "menu-install-command-line-tool"), InstallCommandLineTool),
                MenuItem::separator(),
                MenuItem::os_submenu(say!(w, "menu-services"), gpui::SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-hide-app"), HideApp),
                MenuItem::action(say!(w, "menu-hide-others"), HideOthers),
                MenuItem::action(say!(w, "menu-show-all"), ShowAll),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-quit"), Quit),
            ]),
        ),
        (
            BarMenu::File,
            Menu::new(say!(w, "menu-file")).items([
                MenuItem::action(say!(w, "menu-new-stack"), NewStack).disabled(s.running),
                MenuItem::action(say!(w, "menu-new-batch"), OpenBatch).disabled(s.running),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-add-frames"), AddImages).disabled(s.running),
                MenuItem::action(say!(w, "menu-add-folder"), AddFolder).disabled(s.running),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-export"), SaveResult).disabled(!s.can_export),
                MenuItem::action(say!(w, "menu-export-again"), ExportAgain).disabled(!s.can_export),
                MenuItem::action(say!(w, "menu-export-grow-sequence"), ExportGrowSequence)
                    .disabled(!s.has_result || s.running),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-show-frames-in-finder"), ShowFramesInFinder)
                    .disabled(!s.frames),
                MenuItem::action(say!(w, "menu-close-window"), CloseWindow),
            ]),
        ),
        (
            BarMenu::Edit,
            Menu::new(say!(w, "menu-edit")).items([
                MenuItem::action(undo, Undo).disabled(!s.can_undo),
                MenuItem::action(redo, Redo).disabled(!s.can_redo),
                MenuItem::separator(),
                MenuItem::os_action(
                    say!(w, "menu-cut"),
                    gpui_kit::component::input::Cut,
                    gpui::OsAction::Cut,
                ),
                MenuItem::os_action(
                    say!(w, "menu-copy"),
                    gpui_kit::component::input::Copy,
                    gpui::OsAction::Copy,
                ),
                MenuItem::os_action(
                    say!(w, "menu-paste"),
                    gpui_kit::component::input::Paste,
                    gpui::OsAction::Paste,
                ),
                MenuItem::action(say!(w, "menu-select-all-frames"), SelectAllFrames)
                    .disabled(!s.frames),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-rename-stack"), RenameStack).disabled(!s.stacked),
                MenuItem::action(say!(w, "menu-rename-render"), RenameRender)
                    .disabled(s.picked.is_none()),
                MenuItem::action(say!(w, "menu-remove-all-frames"), ClearFrames)
                    .disabled(!s.frames),
            ]),
        ),
        (
            BarMenu::View,
            Menu::new(say!(w, "menu-view")).items([
                layout(
                    if s.painting { say!(w, "menu-painted") } else { say!(w, "menu-result") },
                    0,
                    Look::Result,
                    Box::new(ViewResult),
                ),
                layout(say!(w, "menu-split"), 1, Look::Split, Box::new(ViewCompare)),
                layout(say!(w, "menu-side-by-side"), 2, Look::SideBySide, Box::new(ViewSideBySide)),
                layout(
                    say!(w, "menu-top-and-bottom"),
                    3,
                    Look::TopAndBottom,
                    Box::new(ViewStacked),
                ),
                layout(say!(w, "menu-flicker"), 4, Look::Flicker, Box::new(Flicker)),
                MenuItem::action(say!(w, "menu-swap-sides"), SwapSides).disabled(!s.comparing),
                MenuItem::separator(),
                MenuItem::action(
                    show_hide(
                        s.map == Some(true),
                        say!(w, "menu-hide-depth-map"),
                        say!(w, "menu-show-depth-map"),
                    ),
                    ToggleMap,
                )
                .disabled(s.map.is_none()),
                MenuItem::action(
                    show_hide(
                        s.contribution == Some(true),
                        say!(w, "menu-hide-contribution"),
                        say!(w, "menu-show-contribution"),
                    ),
                    ToggleContribution,
                )
                .disabled(s.contribution.is_none()),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-zoom-in"), ZoomIn).disabled(!s.has_image),
                MenuItem::action(say!(w, "menu-zoom-out"), ZoomOut).disabled(!s.has_image),
                MenuItem::action(say!(w, "menu-zoom-to-fit"), ZoomToFit).disabled(!s.has_image),
                MenuItem::action(say!(w, "menu-actual-size"), ZoomTo100).disabled(!s.has_image),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-stacking"), ShowStacking).checked(!s.library),
                MenuItem::action(say!(w, "menu-library"), ShowLibrary).checked(s.library),
                MenuItem::separator(),
                MenuItem::action(
                    show_hide(s.sidebar, say!(w, "menu-hide-frames"), say!(w, "menu-show-frames")),
                    ToggleSidebar,
                ),
                MenuItem::action(
                    show_hide(
                        s.inspector == Some(true),
                        say!(w, "menu-hide-inspector"),
                        say!(w, "menu-show-inspector"),
                    ),
                    ToggleInspector,
                )
                .disabled(s.inspector.is_none()),
                MenuItem::action(say!(w, "menu-focus-mode"), ToggleFocusMode).checked(s.focus),
                MenuItem::action(
                    if s.fullscreen {
                        say!(w, "menu-exit-full-screen")
                    } else {
                        say!(w, "menu-enter-full-screen")
                    },
                    ToggleFullScreen,
                ),
            ]),
        ),
        (
            BarMenu::Stack,
            Menu::new(say!(w, "menu-stack")).items([
                MenuItem::action(say!(w, "menu-run-stack"), RunStack).disabled(!s.can_stack),
                MenuItem::action(
                    match s.cancel {
                        Some(CancelItem::Batch) => say!(w, "menu-cancel-batch"),
                        Some(CancelItem::Sequence) => say!(w, "menu-cancel-sequence"),
                        Some(CancelItem::Stack) | None => say!(w, "menu-cancel-stack"),
                    },
                    CancelStack,
                )
                .disabled(s.cancel.is_none()),
                revert_item,
                MenuItem::action(say!(w, "menu-render-info"), RenderInfo).disabled(!s.info),
                MenuItem::separator(),
                MenuItem::action(
                    if s.painting {
                        say!(w, "menu-done-retouching")
                    } else {
                        say!(w, "menu-retouch")
                    },
                    TogglePaint,
                )
                .disabled(!s.can_retouch),
                MenuItem::action(
                    if s.picked == Some(true) {
                        say!(w, "menu-unstar-render")
                    } else {
                        say!(w, "menu-star-render")
                    },
                    TogglePick,
                )
                .disabled(s.picked.is_none()),
                MenuItem::separator(),
                MenuItem::action(
                    if s.excludes {
                        say!(w, "menu-exclude-selected-frames")
                    } else {
                        say!(w, "menu-include-selected-frames")
                    },
                    ToggleExcluded,
                )
                .disabled(!s.frames),
                MenuItem::action(say!(w, "menu-include-only-selected-frames"), IncludeOnlySelected)
                    .disabled(!s.frames),
                MenuItem::action(say!(w, "menu-include-all-frames"), IncludeAllFrames)
                    .disabled(!s.frames || s.all_included),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-previous-frame"), PreviousFrame)
                    .disabled(!s.has_result),
                MenuItem::action(say!(w, "menu-next-frame"), NextFrame).disabled(!s.has_result),
            ]),
        ),
        // macOS adds Fill, Center, Move & Resize and the window list.
        (
            BarMenu::Window,
            Menu::new(say!(w, "menu-window")).items([
                MenuItem::action(say!(w, "menu-minimize"), MinimizeWindow),
                MenuItem::action(say!(w, "menu-zoom"), ZoomWindow),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-bring-all-to-front"), BringAllToFront),
            ]),
        ),
        (
            BarMenu::Help,
            Menu::new(say!(w, "menu-help")).items([
                MenuItem::action(say!(w, "menu-app-help"), OpenHelp),
                MenuItem::action(say!(w, "menu-keyboard-shortcuts"), ShowShortcuts),
                MenuItem::action(say!(w, "menu-choosing-a-method"), ChoosingAMethod),
                try_sample_item(s, w),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-report-a-problem"), ReportProblem),
                MenuItem::action("Benchmark…", RunBenchmark),
                MenuItem::action(say!(w, "menu-save-diagnostics"), SaveDiagnostics),
                MenuItem::action(say!(w, "menu-show-log-folder"), ShowLogFolder)
                    .disabled(!s.log_folder),
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-release-notes"), ReleaseNotes),
            ]),
        ),
    ]
}

/// Help's Try a Sample Stack: a submenu by name for two or more samples,
/// else one item for the one or the website's.
fn try_sample_item(s: &MenuState, w: &DesktopWords) -> MenuItem {
    let name = say!(w, "menu-try-sample");
    match s.sample_stacks.as_slice() {
        [] => MenuItem::action(name, TrySample(None)).disabled(!s.can_try_sample),
        [one] => MenuItem::action(name, TrySample(Some(*one))).disabled(!s.can_try_sample),
        many => MenuItem::submenu(Menu::new(name).items(many.iter().map(|&sample| {
            MenuItem::action(sample.name(), TrySample(Some(sample))).disabled(!s.can_try_sample)
        }))),
    }
}

#[cfg(test)]
mod key_text_tests {
    use super::key_text;

    #[test]
    fn writes_keys_for_the_platform() {
        if cfg!(target_os = "macos") {
            assert_eq!(key_text("⇧⌘E"), "⇧⌘E");
        } else {
            assert_eq!(key_text("⇧⌘E"), "Ctrl+Shift+E");
            assert_eq!(key_text("⌃⌘S / ⌃⌘I"), "Ctrl+Alt+S / Ctrl+Alt+I");
            assert_eq!(key_text("⌘↩"), "Ctrl+Enter");
            assert_eq!(key_text("⌘⌫ / ⌦"), "Ctrl+Backspace / Delete");
            assert_eq!(key_text("⌥-click"), "Alt-click");
            assert_eq!(key_text("⌘-scroll"), "Ctrl-scroll");
        }
    }
}
