//! Try a Sample Stack's catalog and copy (`deptheos_app::frames::sample`),
//! as on the desktop: the app gives where its bundle carries a sample and
//! where copies go (Imports).

use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::engine::work::Lane;
use deptheos_app::frames::sample::{Sample, SampleStack};
use deptheos_app::words::ios::Line;

use crate::request::Ask;
use crate::{Engine, EngineError, Request, Result};

/// One of the sample stacks the app may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum BundledSample {
    Hornet,
    CircuitBoard,
}

impl From<BundledSample> for SampleStack {
    fn from(s: BundledSample) -> Self {
        match s {
            BundledSample::Hornet => Self::Hornet,
            BundledSample::CircuitBoard => Self::CircuitBoard,
        }
    }
}

impl From<SampleStack> for BundledSample {
    fn from(s: SampleStack) -> Self {
        match s {
            SampleStack::Hornet => Self::Hornet,
            SampleStack::CircuitBoard => Self::CircuitBoard,
        }
    }
}

/// The samples in the order they are offered: [`SampleStack::ALL`].
#[uniffi::export]
pub fn bundled_samples() -> Vec<BundledSample> {
    SampleStack::ALL.into_iter().map(BundledSample::from).collect()
}

/// `sample`'s folder in the app's bundle, under its `samples`.
#[uniffi::export]
pub fn sample_folder(sample: BundledSample) -> String {
    SampleStack::from(sample).bundled_folder().to_string()
}

/// `sample`'s name: its copy's folder, and so its stack's. Not translated.
#[uniffi::export]
pub fn sample_name(sample: BundledSample) -> String {
    SampleStack::from(sample).name().to_string()
}

/// A sample's copy: its folder, and whether a whole one was there.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SampleCopied {
    pub folder: PathBuf,
    pub reused: bool,
}

#[uniffi::export]
impl Engine {
    /// `sample`'s copy in `into` (Imports): one already whole, or a new one
    /// made whole or not at all from `from`, once space is checked against
    /// `free` (what the system counts as free for it). Every failure leads
    /// with what didn't happen.
    pub async fn copy_sample(
        self: Arc<Self>,
        request: Arc<Request>,
        sample: BundledSample,
        from: PathBuf,
        into: PathBuf,
        free: Option<u64>,
    ) -> Result<SampleCopied> {
        let copy = move |e: &Engine| {
            let words = e.engine.words();
            let not_copied =
                |err: &deptheos_app::Error| EngineError::led(Line::SampleNotCopied, err, words);
            let Some(found) = Sample::read(sample.into(), &from) else {
                let missing = deptheos_app::Error::InvalidState(format!(
                    "the sample {} isn't in {}",
                    sample_name(sample),
                    from.display()
                ));
                return Err(not_copied(&missing));
            };
            let copied = found.copy_into(&into, free).map_err(|err| not_copied(&err))?;
            Ok(SampleCopied { folder: copied.folder, reused: copied.reused })
        };
        self.pool(&request, Lane::Background, Ask::Own("sample-copy"), copy).await
    }
}
