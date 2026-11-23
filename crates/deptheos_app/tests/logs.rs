//! The installed log, end to end: its own test binary, since `install`
//! sets the process's subscriber and panic hook once.

use deptheos_app::engine::logs;

#[test]
fn header_levels_redaction_panics_and_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let app = logs::App { name: "Deptheos (test)", version: "1.2.3", build: "abc1234 debug" };
    let installed = logs::install(&app, Some(dir.path()));
    assert!(std::ptr::eq(installed, logs::install(&app, None)), "installs once");
    let file = installed.current_file().unwrap();

    let home = dirs::home_dir().unwrap();
    tracing::info!("opened {}", home.join("Pictures/Hornet").display());
    tracing::debug!(target: "deptheos_app::runs::batch", "hidden transition");
    installed.set_verbose(true);
    assert!(installed.is_verbose());
    tracing::debug!(target: "deptheos_app::runs::batch", "shown transition");
    tracing::debug!(target: "wgpu_core::device", "a dependency's debug");
    installed.set_verbose(false);
    tracing::debug!(target: "deptheos_app::runs::batch", "hidden again");

    let panicked = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| panic!("the test's panic"))
        .unwrap()
        .join();
    assert!(panicked.is_err());

    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("Deptheos (test) 1.2.3 (abc1234 debug)"), "{text}");
    assert!(text.contains("os: "), "{text}");
    assert!(text.contains("opened ~/Pictures/Hornet"), "{text}");
    assert!(!text.contains(&*home.to_string_lossy()), "{text}");
    assert!(text.contains("shown transition"), "{text}");
    assert!(!text.contains("hidden"), "{text}");
    assert!(!text.contains("a dependency's debug"), "{text}");
    assert!(text.contains("PANIC thread 'worker'"), "{text}");
    assert!(text.contains("the test's panic"), "{text}");
    assert!(text.contains("backtrace:"), "{text}");

    let zip = dir.path().join("out.zip");
    installed.write_bundle(&zip, &[]).unwrap();
    assert!(std::fs::metadata(&zip).unwrap().len() > 0);
}
