//! Adjust: a render's adjustment sliders and its reframe, set over the
//! stored picture and drawn under them, never written into it (ADR-152).

pub mod copies;
pub mod crop;
pub mod eyedropper;
pub mod histogram;
pub mod machine;
pub mod preview;
pub mod session;
pub mod sliders;

pub use copies::{GpuCopies, RenderMemory};
pub use histogram::{Clipping, Counted, Histogram};
pub use machine::{AdjustClosed, Machine};
pub use session::{AdjustSession, Observer, Update};
pub use sliders::{Group, Panel, Slider};
