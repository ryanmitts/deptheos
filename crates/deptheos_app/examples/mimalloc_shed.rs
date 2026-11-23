//! Whether mimalloc (the desktop's allocator) returns a dropped display
//! pyramid to the system: footprint before, held, dropped, a second later,
//! and after `mi_collect(true)`.
//!
//! `cargo run --release -p deptheos_app --features examples --example mimalloc_shed -- WIDTH HEIGHT`

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn mb() -> String {
    format!("{:.0} MB", deptheos_app::engine::memory::footprint().unwrap_or(0) as f64 / 1e6)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let width: u32 = args.next().and_then(|s| s.parse().ok()).expect("WIDTH");
    let height: u32 = args.next().and_then(|s| s.parse().ok()).expect("HEIGHT");
    let pixels = width as usize * height as usize;
    let rgb: Vec<u16> =
        (0..pixels * 3).map(|i| (i.wrapping_mul(2_654_435_761) >> 7) as u16).collect();
    println!("{width}x{height}: footprint with the source {}", mb());
    // Made on a worker and dropped here, as the desktop does.
    let (ask, asked) = std::sync::mpsc::channel::<()>();
    let (send, made) = std::sync::mpsc::channel();
    let source = rgb.clone();
    std::thread::spawn(move || {
        for () in asked {
            let shown =
                deptheos_app::engine::present::present_rgb(&source, width, height, |v: u16| {
                    (v >> 8) as u8
                });
            if send.send(shown).is_err() {
                break;
            }
        }
    });
    for round in 1..=2 {
        ask.send(()).unwrap();
        let shown = made.recv().unwrap();
        println!("round {round}: holding a {:.0} MB pyramid: {}", shown.bytes() as f64 / 1e6, mb());
        drop(shown);
        println!("  dropped: {}", mb());
        std::thread::sleep(std::time::Duration::from_secs(1));
        println!("  a second later: {}", mb());
    }
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    // SAFETY: mimalloc's own entry point, safe from any thread.
    let started = std::time::Instant::now();
    unsafe { mi_collect(true) };
    let took = started.elapsed().as_secs_f64() * 1e3;
    println!("after mi_collect(true), {took:.1} ms: {}", mb());
}
