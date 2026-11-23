//! Export… (⌘E) opens the Export sheet for the result on screen with its
//! paint; Export Again (⇧⌘E) reuses the last choices without it. The
//! Library's Export… opens it for a render read for it (`library_export.rs`).
//! Either is the window's one export (`exporting.rs`); choices go in the
//! preferences.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::frames::names::file_label;
use deptheos_app::library::export::{strip_extension, untaken_path, with_extension};
use deptheos_app::library::history::Render;
use deptheos_app::{CompletedStack, Explanation};
use gpui::prelude::*;
use gpui::{App, Context, Window};
use gpui_kit::component::WindowExt as _;

use super::exporting::Written;
use super::sheet::ExportSheet;
use super::{AppView, Sheet};
use crate::export_dialog::{
    ExportChoice, ExportColor, ExportDialog, ExportDialogEvent, ExportInfo, ExportPlace, today,
};
use crate::prefs::{prefs, update_prefs};
use crate::words::{Wording as _, say};
use deptheos_app::library::export::NameParts;

/// What the Export sheet exports.
#[derive(Clone)]
pub(super) enum ExportOf {
    /// The result on screen, with its paint.
    Shown,
    /// A render read for the Library's Export…, its paint, and whether it had any.
    Render { render: Box<Render>, stack: Arc<CompletedStack>, retouched: bool },
}

impl AppView {
    /// A result can be exported now: none during a stack or another export.
    pub(crate) fn can_export(&self, cx: &App) -> bool {
        self.has_result(cx) && !self.is_exporting() && !self.stacking.read(cx).is_running()
    }

    /// What the sheet says about what it exports.
    fn export_info(&self, of: &ExportOf, cx: &App) -> Option<ExportInfo> {
        let renders = self.renders.read(cx);
        let (stack, render, retouched) = match of {
            ExportOf::Shown => {
                let stack = self.stacking.read(cx).result()?.clone();
                let render = self.result_doc(cx).and_then(|(_, id)| renders.find(&id).cloned());
                (stack, render, self.paint.read(cx).is_painted())
            }
            ExportOf::Render { render, stack, retouched } => {
                (stack.clone(), Some(render.as_ref().clone()), *retouched)
            }
        };
        let stack_name = match &render {
            Some(r) => renders.stack_name(&r.stack, cx.wording()),
            None => self.title_parts(cx).name.unwrap_or_else(|| say!(cx, "export-unnamed-stack")),
        };
        let method = match &render {
            Some(r) => r.config.kind().label(crate::words::of(cx)),
            None => stack.config.kind().label(crate::words::of(cx)),
        };
        let (title, subtitle, number) = match &render {
            Some(r) => {
                // The settings start with the method: "Pyramid · S4".
                let settings = r.config.summary(crate::words::of(cx));
                let stack = stack_name.as_str();
                let subtitle = if retouched {
                    say!(cx, "export-subtitle-retouched", stack = stack, settings = settings)
                } else {
                    say!(cx, "export-subtitle", stack = stack, settings = settings)
                };
                let title = say!(cx, "export-title-numbered", number = r.number.to_string());
                (title, subtitle, Some(r.number))
            }
            None => (say!(cx, "export-title"), stack_name.clone(), None),
        };
        let last_dir = prefs(cx).save_dir.clone().filter(|d| d.is_dir());
        Some(ExportInfo {
            title: title.into(),
            subtitle: subtitle.into(),
            thumbnail: render.as_ref().map(|r| (self.renders.clone(), r.id.clone())),
            color: match (stack.is_linear(), stack.profile()) {
                (true, _) => ExportColor::Linear,
                (false, None) => ExportColor::Srgb,
                (false, Some(profile)) => ExportColor::Profile(profile.name().map(Into::into)),
            },
            base_name: prefs(cx).export.file_stem(&NameParts {
                stack: &stack_name,
                render: number,
                method: &method,
                date: &today(),
            }),
            sixteen_bit_source: stack.has_sixteen_bit(),
            frames_dir: stack.paths.first().and_then(|p| p.parent()).map(Path::to_path_buf),
            last_dir,
        })
    }

    /// Export… (⌘E): the sheet.
    pub(crate) fn save_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_export(cx) {
            return;
        }
        self.open_export_sheet(ExportOf::Shown, window, cx);
    }

    /// The Export sheet for `of`, unless one is open.
    pub(super) fn open_export_sheet(
        &mut self,
        of: ExportOf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.sheet, Sheet::None) {
            return;
        }
        let Some(info) = self.export_info(&of, cx) else { return };
        let p = prefs(cx);
        let format = p.save_format_for(info.color.is_linear());
        let (sixteen, place, reveal) = (p.export.sixteen_bit, p.export_place, p.reveal_export);
        let render = match &of {
            ExportOf::Shown => self.result_doc(cx).map(|(_, id)| id),
            ExportOf::Render { render, .. } => Some(render.id.clone()),
        };
        if let Some(id) = render {
            self.renders.update(cx, |r, cx| r.request_thumbnails(vec![id], cx));
        }
        let dialog =
            cx.new(|cx| ExportDialog::new(info, format, sixteen, place, reveal, window, cx));
        let on_event = move |this: &mut Self,
                             event: &ExportDialogEvent,
                             window: &mut Window,
                             cx: &mut Context<Self>| {
            match event {
                ExportDialogEvent::Export(choice) => {
                    this.export(choice.clone(), of.clone(), window, cx)
                }
                ExportDialogEvent::Cancel => true,
            }
        };
        let sheet = |modal| Sheet::Export(ExportSheet { modal, finding: None });
        self.open_modal(dialog, sheet, on_event, window, cx);
    }

    /// Export Again (⇧⌘E): the last choices, no sheet; a taken name gets a
    /// number. Without a folder, the sheet opens.
    pub(crate) fn export_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_export(cx) {
            return;
        }
        let Some(info) = self.export_info(&ExportOf::Shown, cx) else { return };
        let p = prefs(cx);
        let format = p.save_format_for(info.color.is_linear());
        let dir = match p.export_place {
            ExportPlace::Frames => info.frames_dir.clone(),
            ExportPlace::Last => info.last_dir.clone(),
        };
        let (Some(dir), Some(name)) = (dir, with_extension(&info.base_name, format)) else {
            self.save_result(window, cx);
            return;
        };
        let (sixteen_bit, place, reveal) = (p.export.sixteen_bit, p.export_place, p.reveal_export);
        // The free name is found off this thread (shares can take seconds),
        // the latest Export Again winning; written only if the same render is
        // still shown and exportable.
        let shown = self.result_doc(cx).map(|(_, id)| id);
        cx.spawn_in(window, async move |this, cx| {
            let found = crate::work::interactive("export-again-name", {
                let dir = dir.clone();
                move || untaken_path(&dir, strip_extension(&name), format.extension(), Path::exists)
            });
            let Ok(path) = found.await else { return };
            this.update_in(cx, |this, window, cx| {
                if !this.can_export(cx) || this.result_doc(cx).map(|(_, id)| id) != shown {
                    tracing::debug!("export again: the render shown changed meanwhile");
                    return;
                }
                let name = file_label(&path);
                let choice = ExportChoice { format, sixteen_bit, place, name, dir, reveal };
                this.write_export(choice, ExportOf::Shown, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Export as the sheet's `choice`, never over a file (a taken name gets a
    /// number); the sheet closes once the name is found.
    fn export(
        &mut self,
        choice: ExportChoice,
        of: ExportOf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The free name is found off this thread by a task the sheet holds,
        // Export busy meanwhile: closing drops it. The sheet stays open.
        let (dir, name, format) = (choice.dir.clone(), choice.name.clone(), choice.format);
        let finding = cx.spawn_in(window, async move |this, cx| {
            let found = crate::work::interactive("export-preflight", move || {
                untaken_path(&dir, strip_extension(&name), format.extension(), Path::exists)
            });
            let Ok(path) = found.await else {
                // Export can be pressed again.
                this.update(cx, |this, cx| {
                    if let Sheet::Export(sheet) = &mut this.sheet
                        && let Some(finding) = sheet.finding.take()
                    {
                        finding.detach();
                        sheet.modal.dialog.update(cx, |d, cx| d.set_finding(false, cx));
                    }
                })
                .ok();
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if path != choice.path() {
                    tracing::debug!("export: the name chosen is taken, so it is numbered");
                }
                let choice = ExportChoice { name: file_label(&path), ..choice };
                // This task is the sheet's: let it finish as the sheet goes.
                if let Sheet::Export(sheet) = &mut this.sheet
                    && let Some(finding) = sheet.finding.take()
                {
                    finding.detach();
                }
                this.set_sheet(Sheet::None);
                window.close_dialog(cx);
                this.write_export(choice, of, window, cx);
            })
            .ok();
        });
        match &mut self.sheet {
            Sheet::Export(sheet) => {
                if sheet.finding.replace(finding).is_some() {
                    tracing::debug!("export: the name lookup before is replaced");
                }
                sheet.modal.dialog.update(cx, |d, cx| d.set_finding(true, cx));
            }
            // Pressed with no Export sheet up: nothing to export into.
            _ => drop(finding),
        }
        false
    }

    /// Remember `choice` for next time, then save.
    fn write_export(
        &mut self,
        choice: ExportChoice,
        of: ExportOf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The format is remembered per kind: RAW and DNG stacks keep their own.
        let info = self.export_info(&of, cx);
        let linear = info.as_ref().is_some_and(|info| info.color.is_linear());
        let sixteen_bit_source = info.is_none_or(|info| info.sixteen_bit_source);
        update_prefs(cx, |p| {
            p.remember_save_format(linear, choice.format);
            // JPEG, DNG and 8-bit stacks fix the depth; the choice for others stays.
            if !choice.format.is_depth_fixed(sixteen_bit_source) {
                p.export.sixteen_bit = choice.sixteen_bit;
            }
            p.export_place = choice.place;
            p.reveal_export = choice.reveal;
            p.save_dir = Some(choice.dir.clone());
        });
        let mut options = prefs(cx).save_options();
        options.sixteen_bit = choice.sixteen_bit;
        let path: PathBuf = choice.path();
        let what = match of {
            ExportOf::Render { render, stack, .. } => Written::Render(render.id, stack),
            ExportOf::Shown => Written::Shown,
        };
        self.write_to(what, path, options, window, cx);
    }

    /// Saving to `path` failed: say so, with `again` to export again.
    pub(super) fn export_failed(
        &mut self,
        path: &Path,
        error: &Explanation,
        again: crate::notices::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::notices::{Details, Kind, Subject};
        let folder = path.parent().map(file_label);
        let lead = match folder {
            Some(folder) => say!(cx, "app-export-failed-to", folder = folder),
            None => say!(cx, "app-export-failed"),
        };
        let what =
            crate::notices::sentences(cx.wording(), Some(&lead), error, Details::Diagnostics);
        self.post(Kind::Error, Subject::Export, what, vec![again], window, cx);
    }

    /// Saved: say so, and show it in the Finder if asked, else offer to.
    pub(super) fn exported(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        use crate::notices::{Action, Kind, Subject};
        let message = say!(cx, "export-done", name = file_label(path));
        let reveal = prefs(cx).reveal_export;
        let actions = if reveal {
            cx.reveal_path(path);
            Vec::new()
        } else {
            let shown = path.to_path_buf();
            let label = crate::renders::reveal_label(cx.wording());
            vec![Action::new(label, move |_, cx| cx.reveal_path(&shown))]
        };
        self.post(Kind::Done, Subject::Export, message, actions, window, cx);
    }
}
