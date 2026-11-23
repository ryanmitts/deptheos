//! Deptheos ▸ Install Command Line Tool… (macOS): link
//! `/usr/local/bin/deptheos` to the app's own tool
//! (`Deptheos.app/Contents/MacOS/deptheos`, from script/bundle-mac.sh), so it
//! runs in Terminal and updates with the app. Windows' installer and the
//! Linux packages put it on the PATH themselves.
//!
//! It replaces only a link to another Deptheos app's tool; any other file or
//! link is left for the user. The place is reread just before linking, so
//! the answer is to the question asked.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use gpui_kit::component::WindowExt as _;

use super::AppView;
use crate::notices::{Kind, Subject};
use crate::words::{Wording as _, say};

/// The link: `/usr/local/bin` is on every account's PATH (`/etc/paths`).
const LINK: &str = "/usr/local/bin/deptheos";

/// What Install Command Line Tool… finds.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    /// No `deptheos` beside the app's executable: a build without it.
    NoTool,
    /// The app runs from a disk image or App Translocation's copy: a link
    /// would break.
    NotInstalled,
    /// The link already points at this app's tool.
    Linked,
    /// A file or a foreign link is at the link's place; it isn't replaced.
    Taken,
    /// Link `tool`, replacing another copy's link `replaces` if any.
    /// `outside_applications`: moving the app would break the link.
    Install { tool: PathBuf, replaces: Option<PathBuf>, outside_applications: bool },
}

/// What is at the link's place.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AtLink {
    Nothing,
    /// A link to the tool of a Deptheos app: this one's or another copy's.
    Deptheos(PathBuf),
    /// A file, or a link to anything else.
    Other,
}

/// What is at `link` now.
fn at_link(link: &Path) -> AtLink {
    match std::fs::read_link(link) {
        Ok(target) if is_deptheos_tool(&target) => AtLink::Deptheos(target),
        Ok(_) => AtLink::Other,
        Err(_) if link.symlink_metadata().is_ok() => AtLink::Other,
        Err(_) => AtLink::Nothing,
    }
}

/// `path` is a Deptheos app's tool: `…/Deptheos….app/Contents/MacOS/deptheos`.
fn is_deptheos_tool(path: &Path) -> bool {
    let names: Vec<&std::ffi::OsStr> = path.iter().collect();
    let [.., app, contents, macos, tool] = names.as_slice() else { return false };
    let app = app.to_string_lossy();
    *tool == "deptheos"
        && *macos == "MacOS"
        && *contents == "Contents"
        && app.ends_with(".app")
        && app.starts_with("Deptheos")
}

/// The app of `tool` is in `/Applications` or `~/Applications`, where it
/// stays put.
fn in_applications(tool: &Path, home: Option<&Path>) -> bool {
    tool.starts_with("/Applications")
        || home.is_some_and(|home| tool.starts_with(home.join("Applications")))
}

/// What installing would do for executable `exe` and the link at `link`.
fn find(exe: &Path, link: &Path, home: Option<&Path>) -> Found {
    let tool = exe.with_file_name("deptheos");
    if !tool.is_file() {
        return Found::NoTool;
    }
    let path = tool.to_string_lossy();
    if path.contains("/AppTranslocation/") || path.starts_with("/Volumes/") {
        return Found::NotInstalled;
    }
    let outside_applications = !in_applications(&tool, home);
    match at_link(link) {
        AtLink::Deptheos(target) if target == tool => Found::Linked,
        AtLink::Deptheos(target) => {
            Found::Install { tool, replaces: Some(target), outside_applications }
        }
        AtLink::Other => Found::Taken,
        AtLink::Nothing => Found::Install { tool, replaces: None, outside_applications },
    }
}

/// What the link's place holds when `replaces` was found there.
fn expected(replaces: Option<&Path>) -> AtLink {
    match replaces {
        Some(old) => AtLink::Deptheos(old.to_path_buf()),
        None => AtLink::Nothing,
    }
}

/// Make the link, replacing `replaces` after checking it is still there
/// (`changed` says why not): directly when the folder is writable, else via
/// an administrator prompt saying `prompt`. `Ok(false)` if canceled.
fn install(
    tool: &Path,
    link: &Path,
    replaces: Option<&Path>,
    prompt: &str,
    changed: &str,
) -> std::io::Result<bool> {
    if at_link(link) != expected(replaces) {
        return Err(std::io::Error::other(changed.to_owned()));
    }
    let dir = link.parent().expect("the link is in a folder");
    if can_write_in(dir) {
        if replaces.is_some() {
            std::fs::remove_file(link)?;
        }
        symlink(tool, link)?;
        return Ok(true);
    }
    // Path and prompt go in as arguments, needing no quoting. With nothing to
    // replace, `ln` without -f fails rather than replace what appeared since.
    let ln = match replaces {
        None => "mkdir -p /usr/local/bin && ln -sn",
        Some(_) => "test -L /usr/local/bin/deptheos && ln -sfn",
    };
    let shell = format!(
        "do shell script \"{ln} \" & quoted form of item 1 of argv & \" /usr/local/bin/deptheos\" \
         with prompt (item 2 of argv) with administrator privileges"
    );
    let script = ["on run argv", shell.as_str(), "end run"];
    let mut command = std::process::Command::new("/usr/bin/osascript");
    for line in script {
        command.args(["-e", line]);
    }
    let output = command.arg(tool).arg(prompt).output()?;
    if output.status.success() {
        return Ok(true);
    }
    let error = String::from_utf8_lossy(&output.stderr);
    // AppleScript's "User canceled." (-128).
    if error.contains("(-128)") {
        return Ok(false);
    }
    Err(std::io::Error::other(error.trim().to_owned()))
}

#[cfg(unix)]
use std::os::unix::fs::symlink;

/// Only the macOS menu bar has the item.
#[cfg(not(unix))]
fn symlink(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("the command line tool is linked only on macOS"))
}

/// A file can be made in `dir` (a missing folder can't; the admin prompt
/// makes it). The probe is created new, never opened over an existing name,
/// and only a probe made here is removed.
fn can_write_in(dir: &Path) -> bool {
    let probe = dir.join(format!(".deptheos-probe-{}", std::process::id()));
    let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    if made {
        std::fs::remove_file(&probe).ok();
    }
    made
}

impl AppView {
    pub(super) fn install_command_line_tool(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exe = std::env::current_exe().unwrap_or_default();
        let found = find(&exe, Path::new(LINK), dirs::home_dir().as_deref());
        tracing::debug!("install command line tool: {found:?}");
        let (tool, replaces, outside_applications) = match found {
            Found::NoTool => {
                let message = say!(cx, "cli-tool-missing");
                self.post(Kind::Error, Subject::Alone, message, Vec::new(), window, cx);
                return;
            }
            Found::NotInstalled => {
                let message = say!(cx, "cli-tool-not-installed");
                self.post(Kind::Info, Subject::Alone, message, Vec::new(), window, cx);
                return;
            }
            Found::Linked => {
                let folder = Path::new(LINK).parent().expect("in a folder");
                let message =
                    say!(cx, "cli-tool-already-installed", folder = folder.display().to_string());
                self.post(Kind::Done, Subject::Alone, message, Vec::new(), window, cx);
                return;
            }
            Found::Taken => {
                let message = say!(cx, "cli-tool-taken", link = LINK);
                self.post(Kind::Error, Subject::Alone, message, Vec::new(), window, cx);
                return;
            }
            Found::Install { tool, replaces, outside_applications } => {
                (tool, replaces, outside_applications)
            }
        };
        let mut description = match &replaces {
            Some(old) => say!(
                cx,
                "cli-tool-install-description-replacing",
                link = LINK,
                old = old.display().to_string()
            ),
            None => say!(cx, "cli-tool-install-description", link = LINK),
        };
        // Its own paragraph: the link breaks if the app moves.
        if outside_applications {
            let warning = say!(cx, "cli-tool-outside-applications");
            description = format!("{description}\n\n{warning}");
        }
        let changed = say!(cx, "cli-tool-link-changed", link = LINK);
        let title = say!(cx, "cli-tool-install-title");
        let ok = say!(cx, "cli-tool-install");
        let prompt = say!(cx, "cli-tool-admin-prompt");
        let cancel = say!(cx, "button-cancel");
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (tool, prompt) = (tool.clone(), prompt.clone());
            let (replaces, changed) = (replaces.clone(), changed.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text(ok.clone())
                .cancel_text(cancel.clone())
                .on_ok(move |_, window, cx| {
                    let (tool, prompt) = (tool.clone(), prompt.clone());
                    let (replaces, changed) = (replaces.clone(), changed.clone());
                    let installing = cx.background_executor().spawn(async move {
                        install(&tool, Path::new(LINK), replaces.as_deref(), &prompt, &changed)
                    });
                    window
                        .spawn(cx, async move |cx| {
                            let installed = installing.await;
                            cx.update(|window, cx| {
                                let (kind, text) = match installed {
                                    Ok(true) => {
                                        tracing::info!("installed the command line tool at {LINK}");
                                        (Kind::Done, say!(cx, "cli-tool-installed"))
                                    }
                                    Ok(false) => return,
                                    Err(e) => {
                                        tracing::warn!(
                                            "couldn't install the command line tool: {e}"
                                        );
                                        let lead = say!(cx, "cli-tool-install-failed");
                                        (
                                            Kind::Error,
                                            crate::notices::notice(cx.wording(), &lead, &e).text,
                                        )
                                    }
                                };
                                crate::notices::post(
                                    kind,
                                    Subject::Alone,
                                    text,
                                    Vec::new(),
                                    window,
                                    cx,
                                );
                            })
                            .ok();
                        })
                        .detach();
                    true
                })
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// An app folder with the GUI's executable and, if `tool`, the CLI.
    fn app(dir: &Path, tool: bool) -> PathBuf {
        let macos = dir.join("Deptheos.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("deptheos-gui"), "").unwrap();
        if tool {
            std::fs::write(macos.join("deptheos"), "").unwrap();
        }
        macos.join("deptheos-gui")
    }

    fn install_found(tool: PathBuf, replaces: Option<PathBuf>) -> Found {
        Found::Install { tool, replaces, outside_applications: true }
    }

    #[test]
    fn finds_what_installing_would_do() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("bin/deptheos");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();

        let without = app(&dir.path().join("a"), false);
        assert_eq!(find(&without, &link, None), Found::NoTool);

        let exe = app(&dir.path().join("b"), true);
        let tool = exe.with_file_name("deptheos");
        assert_eq!(find(&exe, &link, None), install_found(tool.clone(), None));

        // Another copy's link is replaced, and named.
        let other = app(&dir.path().join("old"), true).with_file_name("deptheos");
        symlink(&other, &link).unwrap();
        assert_eq!(find(&exe, &link, None), install_found(tool.clone(), Some(other)));

        std::fs::remove_file(&link).unwrap();
        symlink(&tool, &link).unwrap();
        assert_eq!(find(&exe, &link, None), Found::Linked);

        // A plain file there, or a link to anything else, isn't replaced.
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "").unwrap();
        assert_eq!(find(&exe, &link, None), Found::Taken);
        std::fs::remove_file(&link).unwrap();
        symlink(dir.path().join("Cellar/deptheos/1.0/bin/deptheos"), &link).unwrap();
        assert_eq!(find(&exe, &link, None), Found::Taken);
    }

    #[test]
    fn only_a_deptheos_apps_tool_counts_as_ours() {
        let ours = Path::new("/Applications/Deptheos.app/Contents/MacOS/deptheos");
        assert!(is_deptheos_tool(ours));
        assert!(is_deptheos_tool(Path::new("/x/Deptheos 2.app/Contents/MacOS/deptheos")));
        assert!(!is_deptheos_tool(Path::new("/x/Other.app/Contents/MacOS/deptheos")));
        assert!(!is_deptheos_tool(Path::new("/x/Deptheos.app/Contents/MacOS/deptheos-gui")));
        assert!(!is_deptheos_tool(Path::new("/opt/homebrew/bin/deptheos")));
        assert!(!is_deptheos_tool(Path::new("deptheos")));
    }

    /// Outside an Applications folder the install warns that moving breaks
    /// the link.
    #[test]
    fn an_app_outside_applications_is_said_to_be() {
        let home = Path::new("/Users/someone");
        let tool = |p: &str| PathBuf::from(p).join("Deptheos.app/Contents/MacOS/deptheos");
        assert!(in_applications(&tool("/Applications"), Some(home)));
        assert!(in_applications(&tool("/Users/someone/Applications"), Some(home)));
        assert!(!in_applications(&tool("/Users/someone/Downloads"), Some(home)));
        assert!(!in_applications(&tool("/Applications Old"), Some(home)));
        let dir = tempfile::tempdir().unwrap();
        let exe = app(dir.path(), true);
        let link = dir.path().join("deptheos");
        let Found::Install { outside_applications, .. } = find(&exe, &link, Some(dir.path()))
        else {
            panic!()
        };
        assert!(outside_applications);
        let exe = app(&dir.path().join("Applications"), true);
        let Found::Install { outside_applications, .. } = find(&exe, &link, Some(dir.path()))
        else {
            panic!()
        };
        assert!(!outside_applications);
    }

    /// Installing rereads the place first: what was found is replaced,
    /// anything new or changed is left.
    #[test]
    fn install_replaces_only_what_was_found() {
        let dir = tempfile::tempdir().unwrap();
        let tool = app(&dir.path().join("new"), true).with_file_name("deptheos");
        let old = app(&dir.path().join("old"), true).with_file_name("deptheos");
        let link = dir.path().join("deptheos");
        let changed = "it changed";
        let install = |replaces: Option<&Path>| install(&tool, &link, replaces, "", changed);

        assert!(install(None).unwrap());
        assert_eq!(std::fs::read_link(&link).unwrap(), tool);

        std::fs::remove_file(&link).unwrap();
        symlink(&old, &link).unwrap();
        assert!(install(Some(&old)).unwrap(), "another copy's link is replaced");
        assert_eq!(std::fs::read_link(&link).unwrap(), tool);

        // A file appeared where nothing was.
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "mine").unwrap();
        assert_eq!(install(None).unwrap_err().to_string(), changed);
        assert_eq!(install(Some(&old)).unwrap_err().to_string(), changed);
        assert_eq!(std::fs::read(&link).unwrap(), b"mine", "left as it was");
    }

    /// The write probe is made new: a link of its name is never followed.
    #[test]
    fn the_write_probe_never_opens_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        assert!(can_write_in(dir.path()));
        assert!(!can_write_in(&dir.path().join("missing")));
        let target = dir.path().join("target");
        let probe = dir.path().join(format!(".deptheos-probe-{}", std::process::id()));
        symlink(&target, &probe).unwrap();
        assert!(!can_write_in(dir.path()), "the name is taken");
        assert!(!target.exists(), "nothing made through the link");
        assert!(probe.symlink_metadata().is_ok(), "and the link left");
    }

    #[test]
    fn a_translocated_app_is_not_linked() {
        let dir = tempfile::tempdir().unwrap();
        let exe = app(&dir.path().join("AppTranslocation/X"), true);
        assert_eq!(find(&exe, &dir.path().join("deptheos"), None), Found::NotInstalled);
    }
}
