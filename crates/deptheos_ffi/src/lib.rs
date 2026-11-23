//! Swift bindings for the stacking engine (UniFFI, proc-macros only), for
//! the iOS app in `ios/`.
//!
//! Deliberately coarse: a few long calls, each `async`, handed to the
//! engine's threads at once and canceled through its [`Request`] (ADR-143,
//! `request`); quick ones asked while drawing stay synchronous.
//!
//! Frames reach the engine as file paths; the app holds the folder's
//! security scope, so Rust opens the files. DNG goes through the DNG SDK
//! (LibRaw demosaics a mosaic one) and other camera RAW through LibRaw,
//! linear, as on the desktop. Standard images go to the app's
//! [`FrameDecoder`] (ImageIO), which renders straight into a Rust buffer
//! with no copy; so do camera RAW and mosaic DNGs under Apple's
//! [`RawBackend`] (Core Image), arriving as 16-bit Display P3 and stacking
//! as standard images (ADR-131).
//!
//! What comes back is CPU pixels: display images as 8-bit BGRA (Core
//! Graphics reads little-endian 32-bit, alpha first), and files written by
//! the engine's savers.

uniffi::setup_scaffolding!();

mod batch;
mod bench;
mod decode;
mod engine;
mod error;
mod images;
mod library;
mod list;
mod logging;
mod paint;
mod picture;
mod preferences;
mod request;
mod result;
mod roots;
mod sample;
mod settings;
mod stacks;
mod view;
mod words;

pub use batch::*;
pub use bench::*;
pub use decode::*;
pub use engine::*;
pub use error::*;
pub use images::*;
pub use library::*;
pub use list::*;
pub use logging::*;
pub use paint::*;
pub use picture::*;
pub use preferences::*;
pub use request::Request;
pub use result::*;
pub use roots::*;
pub use sample::*;
pub use settings::*;
pub use stacks::*;
pub use view::*;

/// Lock `mutex` even when poisoned: that only means another thread panicked
/// holding it, and one panic shouldn't crash every later call.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
pub use words::*;

#[cfg(test)]
mod tests;
