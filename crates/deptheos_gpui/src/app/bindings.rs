//! The window's actions, from the menus and keys, routed to the view.

use gpui::prelude::*;
use gpui::{Context, Div, Stateful};

use super::AppView;
use super::actions::*;
use crate::viewer::{Look, ViewEvent};

impl AppView {
    /// Bind every action to `root`: Undo and Redo only while paint mode
    /// has a step to take, so their menu items and keys are off otherwise.
    pub(super) fn bind_actions(
        &self,
        root: Stateful<Div>,
        can_undo: bool,
        can_redo: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &TogglePaint, _, cx| this.toggle_paint(cx)))
            .on_action(cx.listener(|this, _: &ToggleAdjust, _, cx| this.toggle_adjust(cx)))
            .on_action(cx.listener(|this, _: &LeaveAdjust, _, cx| this.leave_adjust(cx)))
            .on_action(cx.listener(|this, _: &HoldBefore, _, cx| this.hold_before(true, cx)))
            .on_action(cx.listener(|this, _: &Eyedropper, _, cx| this.toggle_eyedropper(cx)))
            .on_action(cx.listener(|this, _: &CopyAdjustments, _, cx| this.copy_adjustments(cx)))
            .on_action(cx.listener(|this, _: &PasteAdjustments, window, cx| {
                this.paste_adjustments(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetAdjustments, _, cx| {
                this.with_adjust(cx, |a, _| a.reset_all())
            }))
            .on_action(cx.listener(|this, _: &NextSlider, _, cx| {
                this.with_adjust(cx, |a, cx| a.select_next(true, cx))
            }))
            .on_action(cx.listener(|this, _: &PreviousSlider, _, cx| {
                this.with_adjust(cx, |a, cx| a.select_next(false, cx))
            }))
            .on_action(cx.listener(|this, action: &NudgeSlider, _, cx| {
                let steps = f64::from(action.0);
                this.with_adjust(cx, |a, cx| a.nudge(steps, cx))
            }))
            .on_action(cx.listener(|this, _: &AddFolder, window, cx| this.pick_folders(window, cx)))
            .on_action(cx.listener(|this, _: &OpenWindowMenu, window, cx| {
                this.toggle_window_menu(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowFramesInFinder, _, cx| this.show_frames_in_finder(cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseWindow, window, cx| this.close_window(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectAllFrames, _, cx| {
                if this.on_stacking() {
                    this.files.update(cx, |f, cx| f.select_all(cx))
                }
            }))
            .on_action(
                cx.listener(|this, _: &RenameRender, window, cx| this.rename_render(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RenameStack, window, cx| this.rename_stack(window, cx)),
            )
            .on_action(cx.listener(|this, _: &IncludeOnlySelected, _, cx| {
                if this.on_stacking() {
                    this.files.update(cx, |f, cx| f.include_only_selection(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &IncludeAllFrames, _, cx| {
                if this.on_stacking() {
                    this.files.update(cx, |f, cx| f.include_all(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &RenderInfo, _, cx| this.show_render_info(cx)))
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.show_shortcuts(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SaveDiagnostics, window, cx| {
                this.save_diagnostics(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &RunBenchmark, window, cx| this.open_benchmark(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ChoosingAMethod, window, cx| {
                this.choosing_a_method(window, cx)
            }))
            .on_action(cx.listener(|this, action: &TrySample, window, cx| {
                this.try_sample(action.0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePick, window, cx| {
                if this.on_stacking() {
                    this.renders.update(cx, |r, cx| r.toggle_active_pick(window, cx))
                }
            }))
            .on_action(cx.listener(|this, _: &LeavePaint, _, cx| this.leave_paint(cx)))
            .on_action(cx.listener(|this, _: &PeekSource, _, cx| this.peek_source(true, cx)))
            .when(can_undo, |el| {
                el.on_action(cx.listener(|this, _: &Undo, _, cx| {
                    if this.adjusting().is_some() {
                        this.with_adjust(cx, |a, _| a.undo())
                    } else {
                        this.with_paint(cx, |p, cx| p.undo(cx))
                    }
                }))
            })
            .when(can_redo, |el| {
                el.on_action(cx.listener(|this, _: &Redo, _, cx| {
                    if this.adjusting().is_some() {
                        this.with_adjust(cx, |a, _| a.redo())
                    } else {
                        this.with_paint(cx, |p, cx| p.redo(cx))
                    }
                }))
            })
            .on_action(cx.listener(|this, _: &Eraser, _, cx| {
                this.with_paint(cx, |p, cx| p.pick_original(cx))
            }))
            .on_action(cx.listener(|this, _: &BrushTool, _, cx| {
                this.with_paint(cx, |p, cx| p.pick_brush(cx))
            }))
            .on_action(cx.listener(|this, _: &SwapSource, _, cx| {
                this.with_paint(cx, |p, cx| p.swap_source(cx))
            }))
            .on_action(
                cx.listener(|this, _: &NextFrame, window, cx| this.step_frame(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousFrame, window, cx| this.step_frame(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleFocusMode, window, cx| {
                this.toggle_focus_mode(window, cx)
            }))
            .on_action(cx.listener(|this, _: &BrushSmaller, window, cx| {
                this.with_paint(cx, |p, cx| p.scale_size(0.8, window, cx))
            }))
            .on_action(cx.listener(|this, _: &BrushLarger, window, cx| {
                this.with_paint(cx, |p, cx| p.scale_size(1.25, window, cx))
            }))
            .on_action(cx.listener(|this, _: &BrushSofter, window, cx| {
                this.with_paint(cx, |p, cx| p.nudge_hardness(-0.1, window, cx))
            }))
            .on_action(cx.listener(|this, _: &BrushHarder, window, cx| {
                this.with_paint(cx, |p, cx| p.nudge_hardness(0.1, window, cx))
            }))
            .on_action(cx.listener(|this, action: &SetOpacity, window, cx| {
                let opacity = f32::from(action.0) / 10.0;
                this.with_paint(cx, |p, cx| p.set_opacity(opacity, window, cx))
            }))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &AddImages, window, cx| this.add_images(window, cx)))
            .on_action(cx.listener(|this, _: &NewStack, window, cx| this.new_stack(window, cx)))
            .on_action(
                cx.listener(|this, _: &ClearFrames, window, cx| this.confirm_clear(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenBatch, window, cx| this.open_batch(window, cx)))
            .on_action(cx.listener(|this, _: &SaveResult, window, cx| this.save_result(window, cx)))
            .on_action(
                cx.listener(|this, _: &ExportAgain, window, cx| this.export_again(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                if this.confirm_quit(window, cx) {
                    super::quit::quit_now(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExportGrowSequence, window, cx| {
                this.export_grow_sequence(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RunStack, window, cx| this.run_stacking(window, cx)))
            .on_action(cx.listener(|this, _: &CancelStack, window, cx| {
                if this.can_cancel(cx) {
                    this.cancel_run(window, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &RevertWorking, window, cx| this.revert_shown(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowStacking, window, cx| {
                this.set_screen(super::Screen::Stacking, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowLibrary, window, cx| {
                this.set_screen(super::Screen::Library, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, _, cx| {
                this.library.update(cx, |l, cx| l.open_selected(cx))
            }))
            .on_action(cx.listener(|this, _: &LibrarySelectAll, _, cx| {
                this.library.update(cx, |l, cx| l.select_all(cx))
            }))
            .on_action(cx.listener(|this, _: &LibraryDeselect, _, cx| {
                // Esc closes the preview first.
                this.library.update(cx, |l, cx| {
                    if !l.close_preview(cx) {
                        l.deselect(cx)
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &LibraryPreview, _, cx| {
                this.library.update(cx, |l, cx| l.toggle_preview(cx))
            }))
            .on_action(cx.listener(|this, action: &LibraryMove, _, cx| {
                this.library.update(cx, |l, cx| l.step(action.step, action.extend, cx))
            }))
            .on_action(
                cx.listener(|this, _: &LibraryFindStacks, window, cx| this.find_stacks(window, cx)),
            )
            .on_action(cx.listener(|this, _: &LibraryFocusGrid, window, cx| {
                window.focus(&this.focus_handle, cx)
            }))
            .on_action(cx.listener(|this, _: &LibraryDelete, window, cx| {
                this.library.update(cx, |l, cx| l.delete_selected(window, cx))
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleInspector, window, cx| {
                this.toggle_inspector(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleExcluded, _, cx| {
                if this.on_stacking() {
                    this.files.update(cx, |files, cx| files.toggle_excluded(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &ExcludeSelected, _, cx| {
                if this.on_stacking() {
                    this.files.update(cx, |files, cx| files.exclude_selected(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(|v, at| v.zoom_in(at), cx)))
            .on_action(
                cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(|v, at| v.zoom_out(at), cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomToFit, _, cx| this.zoom(|v, _| v.zoom_to_fit(), cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomTo100, _, cx| this.zoom(|v, at| v.zoom_to_100(at), cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleFit100, _, cx| {
                // Held while painting, space pans instead.
                if this.is_retouching() {
                    this.viewer.update(cx, |v, _| v.set_space_pan(true));
                } else {
                    this.zoom(|v, at| v.toggle_fit_100(at), cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ViewResult, window, cx| {
                this.set_view(Look::Result, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleMap, window, cx| this.toggle_map(window, cx)))
            .on_action(cx.listener(|_, _: &ToggleColorManagement, window, cx| {
                super::view::toggle_color_management(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewCompare, window, cx| {
                this.set_view(Look::Split, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewSideBySide, window, cx| {
                this.set_view(Look::SideBySide, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ViewStacked, window, cx| {
                this.set_view(Look::TopAndBottom, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &Flicker, window, cx| {
                    this.set_view(Look::Flicker, window, cx)
                }),
            )
            .on_action(cx.listener(|this, action: &RetouchLayout, window, cx| {
                if this.is_retouching() {
                    this.set_view(action.0, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SwapSides, window, cx| {
                this.view(ViewEvent::SwapSides, window, cx)
            }))
            .on_action(cx.listener(|this, _: &StopCompare, _, cx| {
                if !this.is_retouching() && this.viewer.read(cx).has_compare() {
                    this.stop_render_compare(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleContribution, window, cx| {
                this.toggle_contribution(window, cx)
            }))
            .on_action(cx.listener(|this, _: &About, window, cx| this.show_about(window, cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| {
                this.check_for_updates(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InstallCommandLineTool, window, cx| {
                this.install_command_line_tool(window, cx)
            }))
    }
}
