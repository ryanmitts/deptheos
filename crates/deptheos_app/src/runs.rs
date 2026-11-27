//! Runs: one stack's session over its frames (`session`, through the
//! stacker facade), a batch of them, grow and turntable sequences, and
//! frames written aligned.

pub(crate) mod aligned;
pub mod batch;
pub mod bench;
pub mod probe;
pub mod sequence;
pub mod session;
pub(crate) mod stacker;
