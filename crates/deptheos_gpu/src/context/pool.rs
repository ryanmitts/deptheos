//! Large textures reused instead of made per pass. A texture of the exact
//! size, format and use comes from the pool when one is there. A dropped
//! one waits in its thread's list until that thread next submits, since
//! commands it recorded may still read it, then joins the pool. Outside a
//! draw ([`GpuContext::drawing`]) nothing is recorded, so it joins at once.
//!
//! After lightcraft 5bdb4e7a6 `crates/gpu/src/ctx.rs` (`Buf`, `Retired`,
//! `RenderScope`, `Gpu::pool`, `Gpu::trim`; MIT OR Apache-2.0), for
//! textures, reused at their exact size.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::GpuContext;

/// The most bytes a pool keeps by default; on iOS, where the GPU's memory
/// is the app's, less.
#[cfg(not(target_os = "ios"))]
pub const POOL_LIMIT: u64 = 768 << 20;
#[cfg(target_os = "ios")]
pub const POOL_LIMIT: u64 = 256 << 20;

/// Every context's pool, for [`trim_pools`].
static POOLS: Mutex<Vec<Weak<TexturePool>>> = Mutex::new(Vec::new());

/// Free every context's pooled textures down to `keep` bytes each: memory
/// running short.
pub fn trim_pools(keep: u64) {
    let pools: Vec<_> = {
        let mut all = POOLS.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|p| p.strong_count() > 0);
        all.iter().filter_map(Weak::upgrade).collect()
    };
    for pool in pools {
        pool.trim(keep);
    }
}

/// What a pooled texture is made as; textures are reused only for an equal
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kind {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
}

impl Kind {
    fn bytes(self) -> u64 {
        let texel = self.format.block_copy_size(None).unwrap_or(16);
        u64::from(self.width) * u64::from(self.height) * u64::from(texel)
    }
}

/// A context's free textures, oldest first.
pub(super) struct TexturePool {
    free: Mutex<VecDeque<(Kind, wgpu::Texture)>>,
    pooled: AtomicU64,
    limit: AtomicU64,
}

impl Default for TexturePool {
    fn default() -> Self {
        Self {
            free: Mutex::default(),
            pooled: AtomicU64::new(0),
            limit: AtomicU64::new(POOL_LIMIT),
        }
    }
}

impl TexturePool {
    /// A new pool, which [`trim_pools`] reaches.
    pub(super) fn registered() -> Arc<Self> {
        let pool = Arc::new(Self::default());
        POOLS.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&pool));
        pool
    }

    fn take(&self, kind: Kind) -> Option<wgpu::Texture> {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        let i = free.iter().position(|(k, _)| *k == kind)?;
        let (_, texture) = free.remove(i)?;
        self.pooled.fetch_sub(kind.bytes(), Ordering::Relaxed);
        Some(texture)
    }

    fn give(&self, kind: Kind, texture: wgpu::Texture) {
        self.free.lock().unwrap_or_else(|e| e.into_inner()).push_back((kind, texture));
        self.pooled.fetch_add(kind.bytes(), Ordering::Relaxed);
        self.trim(self.limit.load(Ordering::Relaxed));
    }

    /// Free the oldest until at most `keep` bytes stay.
    fn trim(&self, keep: u64) {
        let mut dropped = Vec::new();
        {
            let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
            while self.pooled.load(Ordering::Relaxed) > keep {
                let Some((kind, texture)) = free.pop_front() else { break };
                self.pooled.fetch_sub(kind.bytes(), Ordering::Relaxed);
                dropped.push(texture);
            }
        }
        for texture in dropped {
            texture.destroy();
        }
    }
}

/// Textures this thread dropped since its last submit, with their pools.
struct Retired(RefCell<Vec<(Weak<TexturePool>, Kind, wgpu::Texture)>>);

impl Drop for Retired {
    /// At thread exit wgpu's own state may be gone: dropping them is left
    /// to the device.
    fn drop(&mut self) {
        for entry in self.0.get_mut().drain(..) {
            std::mem::forget(entry);
        }
    }
}

thread_local! {
    static RETIRED: Retired = const { Retired(RefCell::new(Vec::new())) };
    /// Draws running on this thread, which may hold unsubmitted commands.
    static DRAWING: Cell<u32> = const { Cell::new(0) };
}

/// This thread's retired textures into their pools.
fn pool_retired() {
    let retired = RETIRED.try_with(|r| std::mem::take(&mut *r.0.borrow_mut())).unwrap_or_default();
    for (pool, kind, texture) in retired {
        if let Some(pool) = pool.upgrade() {
            pool.give(kind, texture);
        }
    }
}

/// A texture from [`GpuContext::pooled_texture`], back to the pool when
/// dropped.
pub struct PooledTexture {
    texture: Option<wgpu::Texture>,
    kind: Kind,
    pool: Weak<TexturePool>,
}

impl PooledTexture {
    /// `texture`, dropped as any texture is rather than pooled.
    pub fn unpooled(texture: wgpu::Texture) -> Self {
        let kind = Kind {
            width: texture.width(),
            height: texture.height(),
            format: texture.format(),
            usage: texture.usage(),
        };
        Self { texture: Some(texture), kind, pool: Weak::new() }
    }

    /// The texture, no longer the pool's.
    pub fn into_texture(mut self) -> wgpu::Texture {
        self.texture.take().expect("a live pooled texture")
    }

    /// Bytes it holds.
    pub fn bytes(&self) -> u64 {
        self.kind.bytes()
    }
}

impl std::ops::Deref for PooledTexture {
    type Target = wgpu::Texture;

    fn deref(&self) -> &wgpu::Texture {
        self.texture.as_ref().expect("a live pooled texture")
    }
}

impl Drop for PooledTexture {
    fn drop(&mut self) {
        let Some(texture) = self.texture.take() else { return };
        let Some(pool) = self.pool.upgrade() else { return };
        let Ok(drawing) = DRAWING.try_with(|d| d.get() > 0) else {
            std::mem::forget(texture);
            return;
        };
        if !drawing {
            pool.give(self.kind, texture);
            return;
        }
        let mut entry = Some((Arc::downgrade(&pool), self.kind, texture));
        let _ = RETIRED.try_with(|r| r.0.borrow_mut().extend(entry.take()));
        if let Some(entry) = entry {
            std::mem::forget(entry);
        }
    }
}

/// A draw on this thread ([`GpuContext::drawing`]): pooled textures
/// dropped during it wait for this thread's next submit.
pub struct Drawing(());

impl Drop for Drawing {
    fn drop(&mut self) {
        let outer = DRAWING.with(|d| {
            d.set(d.get().saturating_sub(1));
            d.get() == 0
        });
        if outer {
            pool_retired();
        }
    }
}

impl GpuContext {
    /// A texture as [`Self::create_texture`] makes it, reused from the pool
    /// when one of the same size, format and use is there.
    pub fn pooled_texture(
        &self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        label: &str,
    ) -> PooledTexture {
        let kind = Kind { width, height, format, usage };
        let texture = self
            .pool
            .take(kind)
            .unwrap_or_else(|| self.create_texture(width, height, format, usage, label));
        PooledTexture { texture: Some(texture), kind, pool: Arc::downgrade(&self.pool) }
    }

    /// Marks a draw on this thread until the guard drops. Everything it
    /// records goes through [`Self::submit`].
    pub fn drawing(&self) -> Drawing {
        DRAWING.with(|d| d.set(d.get() + 1));
        Drawing(())
    }

    /// Bytes the pool holds free.
    pub fn pooled_bytes(&self) -> u64 {
        self.pool.pooled.load(Ordering::Relaxed)
    }

    /// Free pooled textures, oldest first, until at most `keep` bytes stay.
    pub fn trim_pool(&self, keep: u64) {
        self.pool.trim(keep);
    }

    /// The most bytes the pool keeps.
    pub fn set_pool_limit(&self, bytes: u64) {
        self.pool.limit.store(bytes, Ordering::Relaxed);
        self.pool.trim(bytes);
    }

    /// After a submit: this thread's dropped textures may be reused.
    pub(super) fn submitted(&self) {
        pool_retired();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::STORAGE_BINDING;

    #[test]
    fn a_dropped_texture_is_reused_at_its_exact_size() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let format = wgpu::TextureFormat::Rgba32Float;
        let first = ctx.pooled_texture(64, 32, format, USAGE, "pool_test");
        let held = (*first).clone();
        drop(first);
        assert_eq!(ctx.pooled_bytes(), 64 * 32 * 16);
        assert!(*ctx.pooled_texture(64, 33, format, USAGE, "pool_test") != held);
        assert!(*ctx.pooled_texture(64, 32, format, USAGE, "pool_test") == held);
    }

    #[test]
    fn during_a_draw_a_dropped_texture_waits_for_the_next_submit() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let format = wgpu::TextureFormat::R32Float;
        let drawing = ctx.drawing();
        drop(ctx.pooled_texture(16, 16, format, USAGE, "pool_test"));
        assert_eq!(ctx.pooled_bytes(), 0);
        ctx.submit(ctx.encoder("pool_test"));
        assert_eq!(ctx.pooled_bytes(), 16 * 16 * 4);
        drop(drawing);
        ctx.trim_pool(0);
        assert_eq!(ctx.pooled_bytes(), 0);
    }

    #[test]
    fn the_pool_keeps_at_most_its_limit() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        ctx.set_pool_limit(100 * 100 * 4);
        let format = wgpu::TextureFormat::R32Float;
        let (a, b) = (
            ctx.pooled_texture(100, 100, format, USAGE, "pool_test"),
            ctx.pooled_texture(100, 50, format, USAGE, "pool_test"),
        );
        drop(a);
        drop(b);
        assert_eq!(ctx.pooled_bytes(), 100 * 50 * 4, "the oldest went first");
    }
}
