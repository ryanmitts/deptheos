//! The frames a stack is made from: the Library's roots (`sources`), the
//! list and the shots of a folder, their capture times, order and names,
//! their thumbnails, and how camera RAW frames are decoded and converted.

pub(crate) mod autosort;
pub mod capture;
pub(crate) mod conversions;
pub mod folder;
pub mod list;
pub mod names;
pub mod order;
pub(crate) mod raw_backend;
pub mod sample;
pub(crate) mod source_thumbnail;
pub mod sources;
