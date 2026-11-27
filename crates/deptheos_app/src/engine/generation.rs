//! Each render's generation: a number for its stored picture with paint,
//! raised by every paint commit, undo, redo and reset. The reduced copy and
//! the local tone's planes are made for one generation
//! ([`crate::adjust::GpuCopies`]), so whatever changes the pixels makes them
//! again and nothing else does. Numbers are unique for the launch.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::library::ids::RenderId;

/// One generation of a render's stored picture with paint. `ON_DISK` is
/// the picture as it opened, before any change this launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Generation(u64);

impl Generation {
    pub const ON_DISK: Self = Self(0);
}

/// Every render's current generation, shared by paint and Adjust.
#[derive(Debug, Default)]
pub struct Generations {
    next: AtomicU64,
    of: Mutex<HashMap<RenderId, Generation>>,
}

impl Generations {
    /// `render`'s generation now.
    pub fn of(&self, render: &RenderId) -> Generation {
        let of = self.of.lock().unwrap_or_else(|e| e.into_inner());
        of.get(render).copied().unwrap_or(Generation::ON_DISK)
    }

    /// A new generation for `render`, whose pixels have just changed.
    pub fn raise(&self, render: &RenderId) -> Generation {
        let generation = Generation(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let mut of = self.of.lock().unwrap_or_else(|e| e.into_inner());
        of.insert(render.clone(), generation);
        tracing::debug!("render {}: generation {}", render.as_str(), generation.0);
        generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_change_is_a_new_generation_of_its_render_only() {
        let generations = Generations::default();
        let (a, b) = (RenderId::for_tests("a"), RenderId::for_tests("b"));
        assert_eq!(generations.of(&a), Generation::ON_DISK);
        let first = generations.raise(&a);
        assert_ne!(first, Generation::ON_DISK);
        assert_eq!(generations.of(&a), first);
        assert_eq!(generations.of(&b), Generation::ON_DISK);
        let second = generations.raise(&a);
        assert_ne!(second, first);
        assert_ne!(generations.raise(&b), second, "numbers are unique for the launch");
    }
}
