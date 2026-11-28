//! What the viewer shows: a file, the view (Compare and its layouts, a
//! pinned render, the depth map), zoom and its field, and the overlay.

use std::path::{Path, PathBuf};

use deptheos_app::Overlay;
use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::RenderId;
use gpui::{App, Context, Entity, Focusable, Window};
use gpui_kit::component::input::{InputEvent, InputState};

use super::AppView;
use crate::files::FileId;
use crate::notices::{Kind, Subject};
use crate::viewer::{Look, Pick, Source, ViewAction, ViewEvent, ViewTransformController};
use crate::words::{Wording as _, say};

impl AppView {
    pub(super) fn sync_zoom_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.zoom_input.read(cx);
        if input.focus_handle(cx).is_focused(window) {
            return;
        }
        let viewer = self.viewer.read(cx);
        let zoom = viewer.zoom_percent();
        let text = match zoom {
            0 => String::new(),
            _ if viewer.is_fitted() => say!(cx, "viewer-zoom-fit"),
            _ => say!(cx, "unit-percent", value = zoom.to_string()),
        };
        if input.value() != text.as_str() {
            self.zoom_input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    pub(super) fn on_zoom_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Clicked into: the whole value, ready to be typed over.
            InputEvent::Focus => input.update(cx, |input, cx| input.select_all(window, cx)),
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value();
                // The number, without the unit shown round it ("80%").
                let number = |c: char| c.is_ascii_digit() || c == '.';
                let digits = text.trim_matches(|c: char| !number(c));
                if let Ok(percent) = digits.parse::<f32>()
                    && percent > 0.0
                {
                    // Typed: about the pane's center, wherever the pointer is.
                    self.zoom(|v, _| v.zoom_to_percent(percent, None), cx);
                }
                // Back to the viewer's keys; the blur shows the zoom.
                window.focus(&self.focus_handle, cx);
            }
            InputEvent::Blur => self.sync_zoom_input(window, cx),
            InputEvent::Change => {}
        }
    }

    /// Escape in the zoom field: leave it, unchanged.
    pub(crate) fn cancel_zoom_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    /// The one way the app changes what the viewer shows (ADR-149); its
    /// answers (a frame to show, or reload with its overlay) are done here.
    pub(crate) fn view(&mut self, event: ViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let actions = self.viewer.update(cx, |v, cx| v.on(event, cx));
        for action in actions {
            match action {
                ViewAction::Show(path) => self.show_path(&path, window, cx),
                ViewAction::Reload => self.viewer.update(cx, |v, cx| v.reload(cx)),
                ViewAction::Light { .. } | ViewAction::Unlight => {}
            }
        }
    }

    /// Show a file: a preview before a stack; after, the warped compare
    /// source, alone, compared, or lit on the map as the rail does
    /// ([`Self::scrub_to`]); in Retouch, the frame painted from.
    pub(crate) fn show_file(&mut self, id: FileId, window: &mut Window, cx: &mut Context<Self>) {
        self.load_file(id, false, window, cx);
    }

    /// Compare a file with the result in the last layout.
    pub(crate) fn compare_file(&mut self, id: FileId, window: &mut Window, cx: &mut Context<Self>) {
        self.load_file(id, true, window, cx);
    }

    /// [`Self::show_file`], or with `compared` [`Self::compare_file`].
    fn load_file(
        &mut self,
        id: FileId,
        compared: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return; // no preview churn while stacking
        }
        let Some(path) = self.files.read(cx).path(id).map(Path::to_path_buf) else { return };
        self.files.update(cx, |files, cx| files.highlight(Some(id), window, cx));
        if self.is_retouching() {
            self.paint.update(cx, |p, cx| p.pick_frame(&path, cx));
            return;
        }
        let frame = self.pick_of(&path, cx);
        let event = match compared {
            true => ViewEvent::CompareFrame(frame),
            false => ViewEvent::ShowFrame { frame, put_out: false },
        };
        self.view(event, window, cx);
    }

    /// `path` as the view names it: its shot's frame in the result, else itself.
    fn pick_of(&self, path: &Path, cx: &App) -> Pick<PathBuf> {
        let stacking = self.stacking.read(cx);
        let frame = self.frame_index(path, cx).zip(stacking.result());
        frame
            .and_then(|(index, stack)| crate::viewer::pick(stack, index))
            .unwrap_or(Pick { id: path.to_path_buf(), supplied: false })
    }

    /// `path`'s position in the result's stack.
    fn frame_index(&self, path: &Path, cx: &App) -> Option<usize> {
        let stacking = self.stacking.read(cx);
        let files = self.files.read(cx);
        match files.row_of(path).and_then(|id| files.shot_files(id)) {
            Some(shot) => stacking.frame_index(shot),
            None => stacking.result()?.paths.iter().position(|p| p == path),
        }
    }

    /// Put `path` on screen: a preview before a stack, else the warped compare
    /// source with the view's overlay; its row is highlighted.
    fn show_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.files.read(cx).row_of(path);
        let name = file_label(path);
        let (source, labels) = match self.stacking.read(cx).result() {
            Some(stack) => {
                let source = Source::Compare {
                    frame_index: self.frame_index(path, cx),
                    stack: stack.clone(),
                    path: path.to_path_buf(),
                    overlay: self.viewer.read(cx).overlay(),
                };
                (source, Some((name.clone().into(), say!(cx, "viewer-result").into())))
            }
            None => (Source::File(path.to_path_buf()), None),
        };
        // A source frame takes the compare panes over from a pinned render.
        self.renders.update(cx, |r, cx| r.clear_compare(cx));
        self.highlight_row(file, window, cx);
        self.viewer.update(cx, |v, cx| v.load(file, &name, source, labels, cx));
    }

    /// Show what the bar, a key or the menu chose (Flicker flips each press);
    /// Retouch has no Flicker and its own layout memory.
    pub(super) fn set_view(&mut self, look: Look, window: &mut Window, cx: &mut Context<Self>) {
        self.view(ViewEvent::Look(look), window, cx);
        self.find_compare_source(window, cx);
    }

    /// The view bar's Compare: the last layout, or the result alone. In
    /// Retouch, the source beside the canvas (Split first), or the canvas.
    pub(super) fn toggle_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view(ViewEvent::ToggleCompare, window, cx);
        self.find_compare_source(window, cx);
    }

    /// A comparison needs a source: with none shown, the highlighted file,
    /// else the stack's first frame (not the first included: includes may
    /// have changed).
    fn find_compare_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = self.viewer.read(cx);
        if viewer.mode().comparing().is_none()
            || self.is_retouching()
            || viewer.has_compare()
            || viewer.is_loading()
        {
            return;
        }
        let files = self.files.read(cx);
        let source = files
            .selected(cx)
            .or_else(|| self.stacking.read(cx).first_frame().and_then(|p| files.row_of(p)));
        if let Some(id) = source {
            self.show_file(id, window, cx);
        }
    }

    pub(super) fn compare_render(
        &mut self,
        render: RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(stack) = self.stacking.read(cx).result().cloned() else { return };
        if self.is_processing(cx) {
            return;
        }
        let renders = self.renders.read(cx);
        let other = renders.reader(&render);
        let reference = renders.label(&render, cx.wording()).unwrap_or_default();
        let shown = renders
            .active(cx)
            .and_then(|r| renders.label(r, cx.wording()))
            .unwrap_or_else(|| say!(cx, "compare-result").into());
        let labels = (
            say!(cx, "compare-a", name = reference.to_string()).into(),
            say!(cx, "compare-b", name = shown.to_string()).into(),
        );
        self.view(ViewEvent::CompareRender(render.clone()), window, cx);
        self.renders.update(cx, |r, cx| r.set_compare(render, cx));
        self.files.update(cx, |files, cx| files.highlight(None, window, cx));
        self.viewer.update(cx, |v, cx| {
            v.load(None, &reference, Source::Render { stack, other }, Some(labels), cx)
        });
    }

    /// Stop comparing with a render: the result alone again.
    pub(super) fn stop_render_compare(&mut self, cx: &mut Context<Self>) {
        self.renders.update(cx, |r, cx| r.clear_compare(cx));
        self.viewer.update(cx, |v, cx| v.clear_compare(cx));
    }

    /// Swap roles: the compared render (A) becomes the result shown, the
    /// shown one pinned as reference. List and settings stay.
    pub(super) fn swap_render_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let renders = self.renders.read(cx);
        let (Some(a), Some(b)) = (renders.active(cx).cloned(), renders.compare().cloned()) else {
            return;
        };
        self.renders.update(cx, |r, cx| {
            r.set_compare(a, cx);
            r.open(&b, window, cx);
        });
    }

    /// Compare the result (B) with a pinned render (A), brought into its
    /// geometry, in the last layout, left of or above B. In paint mode it is
    /// painted from instead, the left panel listing renders.
    pub(super) fn compare_with_render(
        &mut self,
        render: RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_retouching() {
            let label = self.renders.read(cx).label(&render, cx.wording()).unwrap_or_default();
            self.set_sources_tab(crate::paint::SourcesTab::Renders, cx);
            self.paint_from_render(&render, label, cx);
        } else {
            self.compare_render(render, window, cx);
        }
    }

    pub(super) fn zoom(
        &mut self,
        f: impl FnOnce(&mut ViewTransformController, Option<(f32, f32)>),
        cx: &mut Context<Self>,
    ) {
        self.viewer.update(cx, |v, cx| v.zoom(f, cx));
    }

    /// Toggle Contribution on the frame on screen: what the result took from
    /// it, the rest dimmed. Nothing over a frame that supplied nothing; none
    /// in Retouch.
    pub(super) fn toggle_contribution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view(ViewEvent::ToggleContribution, window, cx);
    }

    /// Contribution can toggle: a frame in view (not under the map) that
    /// supplied part of the result; never in Retouch.
    pub(super) fn has_contribution(&self, cx: &App) -> bool {
        let overlaid = self.viewer.read(cx).mode().overlaid();
        !self.is_retouching() && overlaid.is_some_and(|o| o.supplied)
    }

    /// Contribution is chosen (outside Retouch): shown, or kept for the next
    /// frame over one that supplied nothing.
    pub(super) fn contribution_on(&self, cx: &App) -> bool {
        let overlaid = self.viewer.read(cx).mode().overlaid();
        !self.is_retouching() && overlaid.is_some_and(|o| o.chosen == Overlay::Contribution)
    }

    /// Toggle the depth map, which needs a result with a map (a pyramid or
    /// wavelet render without frame map has none). The rail lets go of the
    /// map's pointer.
    ///
    /// The light carries across (ADR-149): the map opens lit for a frame
    /// shown with Contribution (in Retouch, the frame painted from); left lit
    /// outside Retouch, it shows that frame with Contribution where it
    /// supplied part. Otherwise it returns to what it came from.
    pub(super) fn toggle_map(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = self.viewer.read(cx);
        let stacking = self.stacking.read(cx);
        let on_screen = viewer
            .shown_frame_index()
            .zip(stacking.result())
            .and_then(|(index, stack)| crate::viewer::pick(stack, index));
        self.leave_map_pointer(cx);
        self.view(ViewEvent::ToggleMap { on_screen }, window, cx);
        if self.is_retouching() && self.viewer.read(cx).mode().is_map() {
            self.paint.update(cx, |p, cx| p.light_map(cx));
        }
    }

    /// The depth map can be shown: a result with a map.
    pub(crate) fn can_show_map(&self, cx: &App) -> bool {
        self.has_result(cx) && self.viewer.read(cx).has_map()
    }
}

/// Toggle color management to compare by eye: the windows' tags, theme
/// and pictures held change at once; the engine's conversion applies to
/// images drawn afterward.
pub(super) fn toggle_color_management(window: &mut Window, cx: &mut App) {
    let on = !deptheos_app::engine::present::color_managed();
    crate::color::set_color_managed(on, window, cx);
    let message = if on {
        say!(cx, "viewer-color-management-on")
    } else {
        say!(cx, "viewer-color-management-off")
    };
    crate::notices::post(Kind::Info, Subject::Alone, message, Vec::new(), window, cx);
    window.refresh();
}
