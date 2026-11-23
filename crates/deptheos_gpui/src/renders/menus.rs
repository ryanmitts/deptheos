//! The tiles' right-click menus, and Show in Finder.

use std::path::Path;

use deptheos_app::library::history::RenderId;
use gpui::{Context, SharedString, WeakEntity, Window};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

use super::{Renders, RendersEvent};
use crate::words::{DesktopWords, say};

/// A context-menu command on the strip.
pub(super) type MenuAction = Box<dyn Fn(&mut Renders, &mut Window, &mut Context<Renders>)>;

/// What a tile's menu offers, beside the render itself.
pub(super) struct TileFlags {
    pub(super) picked: bool,
    pub(super) locked: bool,
    pub(super) can_delete: bool,
    /// A pyramid render: a grow sequence can be made from it.
    pub(super) pyramid: bool,
    /// How many renders Delete Other Renders… would delete.
    pub(super) others: usize,
    /// The render's number, for Revert to #n.
    pub(super) number: u32,
    /// Revert to #n is offered: the working version differs from it, and
    /// Retouch is closed.
    pub(super) revert: bool,
    /// It is the render shown: there is nothing to compare it with.
    pub(super) shown: bool,
}

pub(super) fn tile_menu(
    menu: PopupMenu,
    this: WeakEntity<Renders>,
    id: RenderId,
    flags: TileFlags,
    w: &DesktopWords,
) -> PopupMenu {
    let TileFlags { picked, locked, can_delete, pyramid, others, number, revert, shown } = flags;
    let item = |label: SharedString, enabled: bool, action: MenuAction| {
        entity_item(label, &this, action).disabled(!enabled)
    };
    let with = |f: fn(&mut Renders, &RenderId, &mut Window, &mut Context<Renders>)| -> MenuAction {
        let id = id.clone();
        Box::new(move |this, window, cx| f(this, &id, window, cx))
    };
    // Open shows it, as a click does: the working version stays as it is.
    menu.item(item(
        say!(w, "render-open").into(),
        !locked,
        with(|this, id, w, cx| this.click_tile(id, w, cx)),
    ))
    .item(item(
        say!(w, "render-info").into(),
        true,
        with(|this, id, _, cx| this.set_info(Some(id.clone()), cx)),
    ))
    // Compared with the render shown, so not on that render's own tile.
    .item(item(
        compare_label(w).into(),
        !locked && !shown,
        with(|this, id, _, cx| this.request_compare(id, cx)),
    ))
    .item(item(
        say!(w, "render-export-grow-sequence").into(),
        !locked && pyramid,
        with(|_, id, _, cx| cx.emit(RendersEvent::ExportGrow(id.clone()))),
    ))
    .separator()
    // Its frames and settings into the working version: it asks first
    // when that would lose unstacked changes.
    .item(item(
        say!(w, "render-revert-to", number = number.to_string()).into(),
        !locked && revert,
        with(|_, id, _, cx| cx.emit(RendersEvent::Revert(id.clone()))),
    ))
    .item(item(
        say!(w, "render-rename").into(),
        true,
        with(|this, id, w, cx| this.start_rename(id, w, cx)),
    ))
    .item(item(
        if picked { say!(w, "render-unstar") } else { say!(w, "render-star") }.into(),
        true,
        with(|this, id, w, cx| this.toggle_pick(id, w, cx)),
    ))
    .separator()
    .item(item(
        say!(w, "render-delete").into(),
        can_delete,
        with(|this, id, w, cx| this.delete(vec![id.clone()], w, cx)),
    ))
    // The cleanup once this is the result wanted; starred ones stay.
    .item(item(
        say!(w, "render-delete-others", n = others).into(),
        can_delete && others > 0,
        with(|this, id, w, cx| this.delete_others(id, w, cx)),
    ))
}

/// A menu item that runs `action` on `entity`, while it is there.
pub(super) fn entity_item<E: 'static>(
    label: impl Into<SharedString>,
    entity: &WeakEntity<E>,
    action: impl Fn(&mut E, &mut Window, &mut Context<E>) + 'static,
) -> PopupMenuItem {
    let entity = entity.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        entity.update(cx, |e, cx| action(e, window, cx)).ok();
    })
}

fn compare_label(w: &DesktopWords) -> String {
    if cfg!(target_os = "macos") {
        say!(w, "render-compare-mac")
    } else {
        say!(w, "render-compare")
    }
}

/// Show in Finder, or the platform's file manager.
pub(crate) fn reveal_label(w: &DesktopWords) -> String {
    if cfg!(target_os = "macos") {
        say!(w, "render-show-in-finder")
    } else if cfg!(target_os = "windows") {
        say!(w, "render-show-in-explorer")
    } else {
        say!(w, "render-show-in-file-manager")
    }
}

/// Show Frames in Finder, or in the platform's file manager: the command
/// that shows a stack's frames where they are.
pub(crate) fn show_frames_label(w: &DesktopWords) -> String {
    if cfg!(target_os = "macos") {
        say!(w, "menu-show-frames-in-finder")
    } else if cfg!(target_os = "windows") {
        say!(w, "menu-show-frames-in-explorer")
    } else {
        say!(w, "menu-show-frames-in-file-manager")
    }
}

/// Show `path` in the platform's file manager.
pub(crate) fn reveal(path: &Path) {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let spawned = Command::new("open").arg("-R").arg(path).spawn();
    // Explorer parses its own command line: `/select,"<path>"` as one
    // argument keeps spaces and commas (`arg` would quote the switch too).
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt as _;
        Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn()
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let spawned = Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();

    if let Err(e) = spawned {
        tracing::warn!("could not reveal {}: {e}", path.display());
    }
}
