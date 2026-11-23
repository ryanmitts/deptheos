//! Grow sequences: Export Grow Sequence… on a pyramid render (File menu, a
//! render's menu), its dialog, folder, disk check and notices. Batch's
//! turntable starts at the end of a batch (`AppView::start_turntable` in
//! `batch_after.rs`).

use std::path::PathBuf;

use deptheos_app::StackerConfig;
use deptheos_app::frames::names::file_label;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::history::RenderId;
use deptheos_app::runs::sequence::{self, FrameFormat, GrowOptions};
use gpui::prelude::*;
use gpui::{Context, Entity, Window};

use super::away::RunEnd;
use super::{AppView, Running};
use crate::file_dialog::{file_dialog, pick_folder};
use crate::grow_dialog::{FrameControls, GrowDialog, GrowDialogEvent, GrowInfo};
use crate::notices::{Kind, Subject, Worded, show_folder};
use crate::prefs::prefs;
use crate::sequences::{GrowFrom, GrowJob, SequenceEvent, Sequences};
use crate::words::{Wording as _, say};

/// The sequence frame format closest to an export format (DNG has none:
/// a sequence frame is a rendered picture).
pub(crate) fn frame_format(format: ExportFormat) -> FrameFormat {
    match format {
        ExportFormat::Png => FrameFormat::Png,
        ExportFormat::Jpeg => FrameFormat::Jpeg,
        ExportFormat::Tiff | ExportFormat::Dng => FrameFormat::Tiff,
    }
}

impl AppView {
    /// The render a grow sequence would be made from: `id`, else the one on
    /// screen. `None` when there is none, or it is not a pyramid render.
    pub(crate) fn grow_source(&self, id: Option<&RenderId>, cx: &gpui::App) -> Option<RenderId> {
        let id = id.cloned().or_else(|| self.stacking.read(cx).shown_render().cloned())?;
        let render = self.renders.read(cx).find(&id)?;
        matches!(render.config, StackerConfig::Pyramid(_)).then_some(id)
    }

    /// Export Grow Sequence…: the dialog for render `id` (or the one on
    /// screen).
    pub(crate) fn export_grow_sequence(
        &mut self,
        id: Option<RenderId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) || !matches!(self.sheet, super::Sheet::None) {
            return;
        }
        let Some(id) = self.grow_source(id.as_ref(), cx) else {
            let text = say!(cx, "sequence-pyramid-only");
            self.post(Kind::Info, Subject::Sequence, text, Vec::new(), window, cx);
            return;
        };
        let renders = self.renders.read(cx);
        let Some(render) = renders.find(&id).cloned() else { return };
        let paint = deptheos_app::paint::store::paint_file(&renders.store().render_path(&id));
        let stacked: Vec<PathBuf> = render.stacked().cloned().collect();
        // Frames and paint are checked off this thread (shares are slow),
        // latest-wins.
        cx.spawn_in(window, async move |this, cx| {
            let Ok((missing, has_paint)) =
                crate::work::interactive("sequence-preflight", move || {
                    let missing = sequence::missing_sources(&stacked);
                    (missing, paint.exists())
                })
                .await
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if this.is_processing(cx) || !matches!(this.sheet, super::Sheet::None) {
                    return;
                }
                if !missing.is_empty() {
                    this.refuse_missing(missing, window, cx);
                    return;
                }
                this.open_grow_dialog(id, &render, has_paint, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The Export Grow Sequence dialog for `render`, its frames there.
    fn open_grow_dialog(
        &mut self,
        id: RenderId,
        render: &deptheos_app::library::history::Render,
        has_paint: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stacked = render.stacked().count();
        let info = GrowInfo {
            name: crate::renders::untitled(render.display_name(), cx.wording()),
            frames: stacked,
            width: render.width,
            height: render.height,
            sixteen_bit_source: render.linear,
            has_paint,
        };
        let format = frame_format(crate::prefs::prefs(cx).save_format);
        let frames = FrameControls::new(format, prefs(cx).save_options(), window, cx);
        let dialog = cx.new(|_| GrowDialog::new(info.clone(), frames));
        let on_event = move |this: &mut Self,
                             event: &GrowDialogEvent,
                             window: &mut Window,
                             cx: &mut Context<Self>| {
            if let GrowDialogEvent::Export(options) = event {
                this.pick_grow_folder(id.clone(), info.clone(), *options, window, cx);
            }
            true
        };
        self.open_modal(dialog, super::Sheet::Grow, on_event, window, cx);
    }

    /// Refuse a sequence whose source frames are gone, naming them, with
    /// Locate… to point the history at where they are now.
    fn refuse_missing(
        &mut self,
        missing: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut names: Vec<String> = missing
            .iter()
            .take(3)
            .map(|p| say!(cx, "sequence-missing-name", name = file_label(p)))
            .collect();
        let more = missing.len() - names.len();
        // "a", "b" and "c"; or "a", "b", "c" and 4 more.
        let listed = match (names.pop(), more) {
            (Some(last), 0) if names.is_empty() => last,
            (Some(last), 0) => {
                let names = deptheos_app::words::list(crate::words::of(cx), &names);
                say!(cx, "sequence-missing-names", names = names, last = last)
            }
            (Some(last), more) => {
                names.push(last);
                let names = deptheos_app::words::list(crate::words::of(cx), &names);
                say!(cx, "sequence-missing-names-more", names = names, more = more)
            }
            (None, _) => String::new(),
        };
        let message = say!(cx, "sequence-missing", n = missing.len(), listed = listed);
        let first = missing.into_iter().next();
        let locate = self.action(
            say!(cx, "sequence-locate"),
            move |this, window, cx| match &first {
                Some(first) => this.locate_folder(first, window, cx),
                None => this.locate_missing(window, cx),
            },
            cx,
        );
        self.post(Kind::Error, Subject::Sequence, message, vec![locate], window, cx);
    }

    /// Ask where the sequence goes, check the disk has room, and start it.
    fn pick_grow_folder(
        &mut self,
        id: RenderId,
        info: GrowInfo,
        options: GrowOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = file_dialog(window, prefs(cx).save_dir.as_deref())
            .set_title(say!(cx, "file-dialog-grow-sequence"));
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folder(dialog),
            move |this, picked, _, cx| {
                if let Some(parent) = picked {
                    this.start_grow(id, info, options, parent, cx);
                }
            },
            window,
            cx,
        );
    }

    pub(super) fn start_grow(
        &mut self,
        id: RenderId,
        info: GrowInfo,
        options: GrowOptions,
        parent: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        // The render on screen is already loaded; another is read on the
        // worker.
        let shown =
            self.stacking.read(cx).saved_result().filter(|(_, shown)| *shown == id).map(|(s, _)| s);
        let from = match shown {
            Some(shown) => GrowFrom::Loaded(shown),
            None => GrowFrom::History(self.renders.read(cx).store().clone()),
        };
        let job = GrowJob { from, render: id, name: info.name, parent, options };
        // Paint closes first: a sequence locks what a stack does.
        self.leave_paint(cx);
        let awake = super::run::Awake::new("Deptheos is writing a grow sequence", cx);
        self.set_running(Running::Sequence { _awake: awake }, cx);
        self.sequences.update(cx, |s, cx| s.start_grow(job, cx));
    }

    pub(super) fn on_sequence_event(
        &mut self,
        _: &Entity<Sequences>,
        event: &SequenceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A batch's turntable sequence closes with the batch's notice, and
        // the batch stays locked until then.
        if self.batch_sequence_ended(event, window, cx) {
            return;
        }
        self.leave_paint(cx);
        if matches!(self.running, Running::Sequence { .. }) {
            self.set_running(Running::Idle, cx);
        }
        let (kind, end, message, actions) = match event {
            SequenceEvent::Finished { message, folder } => {
                let actions = vec![show_folder(cx.wording(), folder.clone())];
                (Kind::Done, RunEnd::Sequence, Worded::plain(message.as_str()), actions)
            }
            SequenceEvent::Canceled { message } => {
                (Kind::Info, RunEnd::Canceled, Worded::plain(message.as_str()), Vec::new())
            }
            SequenceEvent::Failed(message) => {
                (Kind::Error, RunEnd::SequenceFailed, message.clone(), Vec::new())
            }
        };
        super::away::tell(end, &message.text, window, cx);
        self.post(kind, Subject::Sequence, message, actions, window, cx);
    }
}
