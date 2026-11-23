//! The diagnostic log from the app's side ([`deptheos_app::engine::logs`]):
//! Help ▸ Show Log Folder, Help ▸ Save Diagnostics… (a zip of logs and
//! recent crash reports for [`SUPPORT_ADDRESS`]), Settings' Verbose logging
//! (until quit), and [`AppView::post`], through which fault notices offer
//! those two. The log is installed in `main`, not in tests, where all this
//! does nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use deptheos_app::engine::logs;
use gpui::{App, Context, SharedString, Window};

use super::AppView;
use crate::file_dialog::{file_dialog, save_file};
use crate::notices::{Action, Kind, Subject, Worded};
use crate::words::{DesktopWords, Wording as _, say};

/// Where diagnostics are emailed: crash reports and logs never go on a
/// public issue. Named in fault notices, after Save Diagnostics…, and in
/// `.github/ISSUE_TEMPLATE/bug.yml` (a test holds it to this).
pub(crate) const SUPPORT_ADDRESS: &str = "support@deptheos.com";

/// Crash reports older than this are left out of a bundle.
const CRASH_REPORT_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The folder this launch logs to, when it has one.
pub(crate) fn log_folder() -> Option<PathBuf> {
    logs::installed().and_then(|l| l.dir())
}

/// Show this launch's log file in the file manager, or the folder if the
/// file is unknown.
pub(crate) fn show_log_folder(cx: &mut App) {
    let Some(logs) = logs::installed() else { return };
    if let Some(path) = logs.current_file().or_else(|| logs.dir()) {
        cx.reveal_path(&path);
    }
}

/// Whether Deptheos's own crates log at `debug`.
pub(crate) fn is_verbose() -> bool {
    logs::installed().is_some_and(|l| l.is_verbose())
}

/// Turn verbose logging on or off until the app quits (it is not saved).
pub(crate) fn set_verbose(on: bool) {
    if let Some(logs) = logs::installed() {
        logs.set_verbose(on);
    }
}

/// "Deptheos Diagnostics 2026-11-11.zip", in the app's language.
fn bundle_name(w: &DesktopWords, date: chrono::NaiveDate) -> String {
    let date = date.format("%Y-%m-%d").to_string();
    format!("{}.zip", say!(w, "diagnostics-file-name", date = date))
}

/// Files in `dir` named for `process` ("deptheos-gui-2026-11-11-…ips")
/// modified since `since`, newest first.
fn crash_reports_in(dir: &Path, process: &str, since: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(process))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let modified = meta.modified().ok().filter(|m| *m >= since)?;
            Some((modified, e.path()))
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, path)| path).collect()
}

/// The most crash dumps a bundle takes, newest first, and the largest: a
/// minidump is a few megabytes, a full dump can be the app's whole memory.
const DUMPS_KEPT: usize = 3;
const DUMP_BYTES: u64 = 256 << 20;

/// This app's crash reports from the last 30 days, named after the
/// executable (`deptheos-gui`): macOS's in `~/Library/Logs/DiagnosticReports`;
/// on Windows the app's own minidumps in `%LOCALAPPDATA%\Deptheos\crashes`
/// ([`logs::dumps`]), at most [`DUMPS_KEPT`] of at most [`DUMP_BYTES`]. None
/// on Linux.
fn crash_reports() -> Vec<PathBuf> {
    let since = SystemTime::now().checked_sub(CRASH_REPORT_AGE).unwrap_or(SystemTime::UNIX_EPOCH);
    let process = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "deptheos-gui".to_owned());
    if cfg!(target_os = "macos") {
        let Some(dir) = dirs::home_dir().map(|h| h.join("Library/Logs/DiagnosticReports")) else {
            return Vec::new();
        };
        return crash_reports_in(&dir, &process, since);
    }
    if cfg!(windows) {
        let Some(dir) = logs::dumps::dir() else { return Vec::new() };
        return dumps(crash_reports_in(&dir, &process, since));
    }
    Vec::new()
}

/// The [`DUMPS_KEPT`] newest of `found` no larger than [`DUMP_BYTES`].
fn dumps(found: Vec<PathBuf>) -> Vec<PathBuf> {
    found
        .into_iter()
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= DUMP_BYTES))
        .take(DUMPS_KEPT)
        .collect()
}

impl AppView {
    /// Post a notice here ([`crate::notices::post`]); a fault's also offers
    /// Save Diagnostics… and Show Log Folder.
    pub(crate) fn post(
        &self,
        kind: Kind,
        subject: Subject,
        text: impl Into<Worded>,
        mut actions: Vec<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Worded { text, fault } = text.into();
        if fault {
            let view = cx.weak_entity();
            actions.extend(crate::notices::fault_actions(cx.wording(), move |window, cx| {
                view.update(cx, |this, cx| this.save_diagnostics(window, cx)).ok();
            }));
        }
        crate::notices::post(kind, subject, text, actions, window, cx);
    }

    /// A notice button that runs `run` on this view.
    pub(crate) fn action(
        &self,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Action {
        let view = cx.weak_entity();
        Action::new(label, move |window, cx| {
            view.update(cx, |this, cx| run(this, window, cx)).ok();
        })
    }

    /// Help ▸ Benchmark…: open the in-app benchmark sheet with the open stack
    /// (if ≥2 included frames) and any bundled samples.
    pub(super) fn open_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::bench_dialog::{BenchDialog, BenchDialogEvent, BenchSource};
        use gpui::AppContext as _;

        let mut sources = Vec::new();
        let included: Vec<PathBuf> =
            self.files.read(cx).included().into_iter().map(|(_, p)| p).collect();
        if included.len() >= 2 {
            let name = included[0]
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "stack".into());
            sources.push(BenchSource {
                label: format!("Current stack: {name} ({} frames)", included.len()),
                stack_name: name,
                paths: included,
            });
        }
        for sample in super::sample::bundled() {
            if let Ok(entries) = std::fs::read_dir(sample.dir()) {
                let mut paths: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                            matches!(
                                e.to_ascii_lowercase().as_str(),
                                "jpg" | "jpeg" | "png" | "tif" | "tiff" | "dng"
                            )
                        })
                    })
                    .collect();
                paths.sort();
                if paths.len() >= 2 {
                    let s = sample.stack();
                    sources.push(BenchSource {
                        label: format!("{} ({} frames · 11 MP)", s.name(), paths.len()),
                        stack_name: s.bundled_folder().into(),
                        paths,
                    });
                }
            }
        }
        let engine = self.stacking.read(cx).engine();
        let dialog = cx.new(|_| BenchDialog::new(engine, sources));
        self.open_modal(
            dialog,
            super::Sheet::Bench,
            |this, event, window, cx| match event {
                BenchDialogEvent::Close => true,
                BenchDialogEvent::SaveJson { filename, json } => {
                    let dialog = file_dialog(window, dirs::desktop_dir().as_deref())
                        .set_title("Save Benchmark JSON")
                        .set_file_name(filename)
                        .add_filter("JSON", &["json"]);
                    let json = json.clone();
                    this.picker.ask(
                        |this: &mut Self| &mut this.picker,
                        || save_file(dialog),
                        move |_, picked, window, cx| {
                            let Some(dest) = picked else { return };
                            let write = crate::work::write("benchmark-json", {
                                let dest = dest.clone();
                                move || std::fs::write(&dest, json)
                            });
                            cx.spawn_in(window, async move |_, _cx| {
                                let res =
                                    write.await.unwrap_or_else(|e| Err(std::io::Error::other(e)));
                                if let Err(err) = res {
                                    tracing::error!("saving benchmark JSON failed: {err}");
                                }
                            })
                            .detach();
                        },
                        window,
                        cx,
                    );
                    false
                }
            },
            window,
            cx,
        );
    }

    /// Save Diagnostics…: ask where, write the zip off the main thread, then
    /// show it (or say why not).
    pub(super) fn save_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Installed without a file (it couldn't be made): nothing to save.
        let Some(logs) = logs::installed().filter(|l| l.dir().is_some()) else {
            let text = say!(cx, "diagnostics-no-log");
            self.post(Kind::Error, Subject::Alone, text, Vec::new(), window, cx);
            return;
        };
        let dialog = file_dialog(window, dirs::desktop_dir().as_deref())
            .set_title(say!(cx, "diagnostics-save-title"))
            .set_file_name(bundle_name(cx.wording(), chrono::Local::now().date_naive()))
            .add_filter(say!(cx, "diagnostics-zip-archive"), &["zip"]);
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || save_file(dialog),
            move |_, picked, window, cx| {
                let Some(dest) = picked else { return };
                tracing::info!("saving diagnostics");
                let write = crate::work::write("diagnostics", {
                    let dest = dest.clone();
                    move || logs.write_bundle(&dest, &crash_reports())
                });
                cx.spawn_in(window, async move |_, cx| {
                    let written = write.await.unwrap_or_else(|e| Err(std::io::Error::other(e)));
                    cx.update(|window, cx| match written {
                        Ok(()) => {
                            let name = dest.file_name().unwrap_or(dest.as_os_str());
                            let name = name.to_string_lossy().into_owned();
                            let saved = say!(
                                cx,
                                "diagnostics-saved",
                                name = name,
                                address = SUPPORT_ADDRESS
                            );
                            crate::notices::post(
                                Kind::Done,
                                Subject::Alone,
                                saved,
                                Vec::new(),
                                window,
                                cx,
                            );
                            cx.reveal_path(&dest);
                        }
                        Err(e) => {
                            tracing::warn!("couldn't save diagnostics: {e}");
                            let message = crate::notices::notice(
                                cx.wording(),
                                &say!(cx, "diagnostics-save-failed"),
                                &e,
                            )
                            .text;
                            crate::notices::post(
                                Kind::Error,
                                Subject::Alone,
                                message,
                                Vec::new(),
                                window,
                                cx,
                            );
                        }
                    })
                    .ok();
                })
                .detach();
            },
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_named_for_the_day() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let w = DesktopWords::english();
        assert_eq!(bundle_name(&w, day), "Deptheos Diagnostics 2026-03-07.zip");
    }

    #[test]
    fn the_bug_form_names_the_address() {
        let form = include_str!("../../../../.github/ISSUE_TEMPLATE/bug.yml");
        assert!(form.contains(SUPPORT_ADDRESS), "bug.yml names another support address");
    }

    /// Only the app's own reports, and only recent ones, newest first.
    #[test]
    fn crash_reports_are_the_apps_recent_ones() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, age_days: u64| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"report").unwrap();
            let when = SystemTime::now() - Duration::from_secs(age_days * 24 * 60 * 60);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            path
        };
        let newer = write("deptheos-gui-2026-11-10-120000.ips", 1);
        let older = write("deptheos-gui-2026-11-01-120000.ips", 10);
        write("deptheos-gui-2026-09-01-120000.ips", 70);
        write("Safari-2026-11-10-120000.ips", 1);
        let since = SystemTime::now() - CRASH_REPORT_AGE;
        assert_eq!(crash_reports_in(dir.path(), "deptheos-gui", since), [newer, older]);
        assert!(crash_reports_in(&dir.path().join("missing"), "deptheos-gui", since).is_empty());
    }

    /// The newest few dumps, none too large to send.
    #[test]
    fn dumps_are_the_newest_few_not_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, age_secs: u64, len: u64| {
            let path = dir.path().join(name);
            let file = std::fs::File::create(&path).unwrap();
            file.set_len(len).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age_secs)).unwrap();
            path
        };
        let a = write("deptheos-gui.exe.1.dmp", 1, 10);
        write("deptheos-gui.exe.2.dmp", 2, DUMP_BYTES + 1);
        let c = write("deptheos-gui.exe.3.dmp", 3, 10);
        let d = write("deptheos-gui.exe.4.dmp", 4, 10);
        write("deptheos-gui.exe.5.dmp", 5, 10);
        let found = crash_reports_in(dir.path(), "deptheos-gui.exe", SystemTime::UNIX_EPOCH);
        assert_eq!(dumps(found), [a, c, d]);
    }

    /// Without the log there is no folder and the switch does nothing; with
    /// it the switch sets the level. The only test installing the
    /// process-wide log.
    #[test]
    fn the_verbose_switch_sets_the_level() {
        assert!(logs::installed().is_none(), "only this test installs the log");
        assert_eq!(log_folder(), None);
        set_verbose(true);
        assert!(!is_verbose());

        let dir = tempfile::tempdir().unwrap().keep();
        let app = logs::App { name: "Deptheos (test)", version: "0", build: "" };
        let installed = logs::install(&app, Some(&dir));
        assert_eq!(log_folder().as_deref(), Some(dir.as_path()));
        set_verbose(true);
        assert!(is_verbose() && installed.is_verbose());
        set_verbose(false);
        assert!(!is_verbose());
        std::fs::remove_dir_all(&dir).ok();
    }
}
