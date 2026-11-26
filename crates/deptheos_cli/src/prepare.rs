//! What every command does before it stacks: the frames, the converter
//! settings and the [`Engine`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use deptheos_app::frames::order::OrderNote;
use deptheos_app::{Converted, CoreError, DngCache, Engine, FrameOrder, RawBackend, Words};

use crate::args::DecodeArgs;
use crate::error::{Refusal, Result};

/// The frames `inputs` name: files as given, and each folder's images in
/// natural name order, as the apps list a folder.
pub(crate) fn frames(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut frames = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let found =
                deptheos_app::frames::folder::list_folder(input).map_err(CoreError::from)?;
            if found.is_empty() {
                return Err(Refusal::NoFrames(input.clone()).into());
            }
            frames.extend(found);
        } else {
            frames.push(input.clone());
        }
    }
    Ok(frames)
}

/// The frames `inputs` name, in `order`, warning when they can't be put
/// in it.
pub(crate) fn ordered_frames(
    inputs: &[PathBuf],
    order: FrameOrder,
    words: &Words,
) -> Result<Vec<PathBuf>> {
    let frames = frames(inputs)?;
    let (frames, note) = deptheos_app::frames::order::ordered(frames, order);
    if let Some(note) = note {
        warn_order(&note, words);
    }
    Ok(frames)
}

/// Warns of `note` on the frames' order, pointing to `--order time` when
/// they were shot in another order than their names.
fn warn_order(note: &OrderNote, words: &Words) {
    let message = note.message(words);
    match note {
        OrderNote::DiffersFromNames => {
            crate::ui::warn(format!("{message}; --order time stacks them as shot"))
        }
        _ => crate::ui::warn(message),
    }
}

/// The engine [`engine`] made, held for [`keep_engine_until_exit`].
static ENGINE: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

/// The engine, with the converter and camera profile folders set from
/// `decode` as the apps set them from their preferences.
pub(crate) fn engine(decode: &DecodeArgs, words: &Words) -> Result<Arc<Engine>> {
    if decode.backend() == RawBackend::AdobeDng {
        deptheos_app::set_converter_executable(decode.dng_converter.clone());
        deptheos_app::set_converter_jobs(decode.jobs.map(|n| n as usize));
        deptheos_app::set_converter_uncompressed(decode.dng_uncompressed);
        // What an earlier run cut short (a second Ctrl-C) left behind.
        DngCache::sweep_default_dir();
    }
    if !decode.camera_profile_folder.is_empty() {
        deptheos_app::set_profile_directories(decode.camera_profile_folder.clone());
    }
    let engine = Engine::new(words.clone())?;
    *ENGINE.lock().unwrap_or_else(PoisonError::into_inner) = Some(engine.clone());
    Ok(engine)
}

/// The engine with no decoding preferences, for a command that reads no
/// frames.
pub(crate) fn plain_engine(words: &Words) -> Result<Arc<Engine>> {
    let engine = Engine::new(words.clone())?;
    *ENGINE.lock().unwrap_or_else(PoisonError::into_inner) = Some(engine.clone());
    Ok(engine)
}

/// Called before exit, however the command ended: deletes the converted
/// frames and leaves the GPU device for the OS ([`Engine::keep_until_exit`]).
pub(crate) fn keep_engine_until_exit() {
    let engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(engine) = engine {
        engine.keep_until_exit();
    }
}

/// Converts `frames` up front, holding the conversions while the result
/// lives: for several stacks over the same frames, or to time a stack
/// without them.
pub(crate) fn convert_first(
    engine: &Engine,
    frames: &[PathBuf],
    backend: RawBackend,
) -> Result<Converted> {
    engine
        .convert_first(frames, backend, crate::interrupt::token(), |current, total| {
            crate::ui::progress!("converting to DNG {current} of {total}");
        })?
        .ok_or(deptheos_app::Error::Canceled.into())
}
