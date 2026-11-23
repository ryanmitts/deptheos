//! Retouching: brushing pixels from another image onto a finished stack.
//! UI-free, so it is tested without a window.

pub(crate) mod brush;
pub(crate) mod canvas;
pub(crate) mod display;
pub(crate) mod doc;
pub(crate) mod frame;
pub(crate) mod log;
pub(crate) mod machine;
pub(crate) mod render;
mod session;
pub(crate) mod source;
pub mod store;
#[cfg(test)]
mod store_kill;
pub(crate) mod stroke;
pub(crate) mod tile;
pub(crate) mod undo;
pub(crate) mod worker;

pub use brush::{
    Brush, DEFAULT_TOLERANCE, Dab, FLOW_RANGE, HARDNESS_RANGE, OPACITY_RANGE, RADIUS_RANGE,
    TOLERANCE_RANGE,
};
pub use canvas::Layer;
pub use display::DisplayPatch;
pub use frame::{Siblings, Told, frame_loader};
pub use machine::{CanvasState, OpenRender, Report, Status};
pub use session::{Observer, PaintSession};
pub use source::SourceKind;
pub use tile::{TILE, TILE_PIXELS, TileData, TileId};
pub use worker::{Command, Failure, Flattened, Output, PaintState, PaintWorker, SourceLoader};

// The brush's insides, for `dab_bench` only.
#[cfg(feature = "examples")]
pub use brush::Spacer;
#[cfg(feature = "examples")]
pub use canvas::Canvas;
#[cfg(feature = "examples")]
pub use stroke::{CoveragePool, Stroke, StrokeSource};
