//! Laying a frame out (images per pane, visible tiles) and painting it.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    BorderStyle, Bounds, ContentMask, Context, Corners, CursorStyle, DispatchPhase, Entity, Hitbox,
    HitboxBehavior, Hsla, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, PinchEvent, Pixels, Point, RenderImage, ScrollWheelEvent, Window, fill, point,
    px, quad, size, transparent_black,
};

use super::input::Gesture;
use super::transform::{PANE_GAP, SplitLayout, split_pane};
use deptheos_app::view::{Light, Mode};

use super::{Layout, Tool, Viewer};
use crate::images::{TileDraw, TiledImage};

/// Half-width of the strip that grabs the compare divider.
const DIVIDER_GRAB: f32 = 12.0;

/// One image painted into one pane.
struct Draw {
    /// The pane: what the image is clipped to, in window coordinates.
    clip: Bounds<Pixels>,
    /// Where the whole image lies, in window coordinates.
    rect: Bounds<Pixels>,
    /// Its visible tiles.
    tiles: Vec<TileDraw>,
}

/// Everything prepaint decided, handed to paint.
pub(super) struct Frame {
    draws: Vec<Draw>,
    /// Compare divider line.
    divider: Option<Bounds<Pixels>>,
    /// Side-by-side pane separators.
    separators: Vec<Bounds<Pixels>>,
    hitbox: Hitbox,
    divider_hitbox: Option<Hitbox>,
    dragging: bool,
    /// The Paint tool's crosshair.
    crosshair: bool,
    held: Rc<Cell<bool>>,
    tracking: Rc<Cell<bool>>,
    pointer: Rc<Cell<Option<Point<Pixels>>>>,
    /// Brush rings: center (window coordinates), radius (logical pixels),
    /// hardness, clip.
    rings: Vec<(Point<Pixels>, f32, f32, Bounds<Pixels>)>,
    preview: Option<Preview>,
    /// Adjust's flat picture: its pane, where it lies, its image.
    flat: Option<(Bounds<Pixels>, Bounds<Pixels>, Arc<RenderImage>)>,
}

/// The paint source under the brush's inner circle, drawn over the canvas
/// to show what the brush would lay down.
struct Preview {
    /// In window coordinates.
    center: Point<Pixels>,
    /// In logical pixels.
    radius: f32,
    /// The canvas pane.
    clip: Bounds<Pixels>,
    tiles: Vec<TileDraw>,
}

impl Viewer {
    /// Lay the frame out: size, zoom animation, images per pane, visible
    /// tiles. Hitboxes go here since GPUI accepts them only in prepaint.
    pub(super) fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        self.atlas.begin_frame(window, cx);
        self.atlas.set_crisp(self.tool != Tool::Paint);
        self.bounds = bounds;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.view.set_pixel_ratio(window.scale_factor());
        self.view.set_container_size(w, h);

        // Wall-clock step, capped so a long pause doesn't teleport the zoom.
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|last| now.duration_since(last).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        if self.view.tick(dt) {
            window.request_animation_frame();
        }

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let mut frame = Frame {
            draws: Vec::new(),
            divider: None,
            separators: Vec::new(),
            hitbox,
            divider_hitbox: None,
            dragging: matches!(self.gesture, Gesture::Pan(_)),
            crosshair: matches!(self.tool, Tool::Paint | Tool::Eyedropper),
            held: self.held.clone(),
            tracking: self.tracking.clone(),
            pointer: self.pointer.clone(),
            rings: Vec::new(),
            preview: None,
            flat: None,
        };

        self.layout(bounds, window, &mut frame);
        self.panes = frame.draws.iter().map(|d| (d.clip, d.rect)).collect();
        if let Some(&(clip, rect)) = self.panes.last() {
            self.sync_adjust_view(clip, rect, cx);
            frame.flat = self.flat().map(|f| (clip, self.flat_rect(f, rect), f.image.clone()));
        }
        // No brush over the depth map.
        if self.tool == Tool::Paint && !self.state.now().is_map() {
            self.place_rings(&mut frame);
            self.place_preview(&mut frame);
        }
        let Self { primary, map, lit_map, secondary, before, atlas, .. } = self;
        atlas.evict(
            [
                primary.as_mut(),
                map.image_mut(),
                lit_map.as_mut(),
                secondary.as_mut(),
                before.as_mut(),
            ]
            .into_iter()
            .flatten(),
        );
        self.make_crisp(cx);
        frame
    }

    /// Make this frame's requested enlarged tiles off the UI thread, each
    /// taken by its image if still wanted, drawn next frame.
    fn make_crisp(&mut self, cx: &mut Context<Self>) {
        let jobs = self.atlas.take_jobs();
        if jobs.is_empty() {
            return;
        }
        tracing::debug!("enlarging {} tiles", jobs.len());
        cx.spawn(async move |this, cx| {
            for job in jobs {
                let made = crate::work::background("crisp-tile", move || {
                    let made = job.make();
                    (made, job)
                })
                .await;
                let (made, job) = match made {
                    Ok(made) => made,
                    Err(e) => {
                        tracing::warn!("a tile couldn't be enlarged: {e}");
                        continue;
                    }
                };
                let taken = this.update(cx, |this, cx| {
                    let Self { primary, map, lit_map, secondary, before, .. } = this;
                    let taken = [
                        primary.as_mut(),
                        map.image_mut(),
                        lit_map.as_mut(),
                        secondary.as_mut(),
                        before.as_mut(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|image| image.take_crisp(&job, made.clone()));
                    if taken {
                        cx.notify();
                    }
                });
                if taken.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The panes and their tiles, into `frame`.
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, frame: &mut Frame) {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let map_shown = match self.state.now() {
            Mode::Map { light: Light::Lit { .. }, .. } => Some(true),
            Mode::Map { light: Light::Whole | Light::Marked { .. }, .. } => Some(false),
            Mode::Result | Mode::Frame { .. } | Mode::Compare(_) => None,
        };
        // A lone frame or peeked paint source flickers on its source side.
        // The Paint tool never swaps: the canvas stays the last pane, where
        // `place_preview` looks.
        let drawn = self.drawn_comparing(true);
        let layout = drawn.map(|(layout, _)| layout);
        let swapped = drawn.is_some_and(|(_, swapped)| swapped) && self.tool != Tool::Paint;
        let before_held = self.holding_before() && self.before.is_some();
        let Self { primary, map, lit_map, secondary, before, atlas, view, split, .. } = self;
        // The map can't be on without one ([`Viewer::toggle_map`]); the
        // result stands in while its picture is remade (`MapPicture::Off`).
        let main = match map_shown {
            Some(true) if lit_map.is_some() => lit_map.as_mut(),
            Some(_) if map.image_mut().is_some() => map.image_mut(),
            Some(_) => primary.as_mut(),
            None if before_held => before.as_mut(),
            None if primary.is_some() => primary.as_mut(),
            None => map.image_mut(),
        };
        let Some(main) = main else { return };
        // The tracked image may have changed size: re-fit first.
        if (main.width, main.height) != (view.image_w, view.image_h) {
            view.set_image_size(main.width, main.height);
        }
        // A compare source is drawn into the result's rect, so another size
        // would stretch: draw the result alone.
        let compare = match layout {
            Some(_) => {
                secondary.as_mut().filter(|c| (c.width, c.height) == (main.width, main.height))
            }
            None => None,
        };
        // Panes follow what is drawn: an unloaded source lays out as one pane.
        view.set_split(match (layout, compare.is_some()) {
            (Some(Layout::SideBySide), true) => SplitLayout::Horizontal,
            (Some(Layout::TopAndBottom), true) => SplitLayout::Vertical,
            _ => SplitLayout::None,
        });

        let device_scale = view.device_scale();
        let (ix, iy, iw, ih) = view.image_rect();
        let mut draw = |pane: Bounds<Pixels>, clip: Bounds<Pixels>, image: &mut TiledImage| {
            let rect = Bounds {
                origin: point(pane.origin.x + px(ix), pane.origin.y + px(iy)),
                size: size(px(iw), px(ih)),
            };
            Draw { clip, rect, tiles: image.draws(atlas, rect, clip, device_scale) }
        };

        // Left (or top) and right (or bottom): the compare source first.
        match (layout, compare) {
            (Some(Layout::Flicker { source }), Some(other)) => {
                frame.draws.push(draw(bounds, bounds, if source { other } else { main }));
            }
            (Some(Layout::Split), Some(other)) => {
                let (first, second) = sides(swapped, main, other);
                let split_x = bounds.origin.x + px(w * *split);
                let left = Bounds::from_corners(bounds.origin, point(split_x, bounds.bottom()));
                let right = Bounds::from_corners(
                    point(split_x, bounds.origin.y),
                    point(bounds.right(), bounds.bottom()),
                );
                frame.draws.push(draw(bounds, left, first));
                frame.draws.push(draw(bounds, right, second));
                let divider = Bounds {
                    origin: point(split_x - px(1.5), bounds.origin.y),
                    size: size(px(3.0), bounds.size.height),
                };
                frame.divider = Some(divider);
                frame.divider_hitbox = Some(window.insert_hitbox(
                    Bounds {
                        origin: point(split_x - px(DIVIDER_GRAB), bounds.origin.y),
                        size: size(px(2.0 * DIVIDER_GRAB), bounds.size.height),
                    },
                    HitboxBehavior::Normal,
                ));
            }
            (Some(Layout::SideBySide), Some(other)) => {
                let (first, last) = sides(swapped, main, other);
                let (pane, second) = split_pane(w);
                let left = Bounds { origin: bounds.origin, size: size(px(pane), px(h)) };
                let right = Bounds {
                    origin: point(bounds.origin.x + px(second), bounds.origin.y),
                    size: size(px(pane), px(h)),
                };
                frame.draws.push(draw(left, left, first));
                frame.draws.push(draw(right, right, last));
                frame.separators.push(Bounds {
                    origin: point(bounds.origin.x + px(pane), bounds.origin.y),
                    size: size(px(2.0 * PANE_GAP), px(h)),
                });
            }
            (Some(Layout::TopAndBottom), Some(other)) => {
                let (first, last) = sides(swapped, main, other);
                let (pane, second) = split_pane(h);
                let top = Bounds { origin: bounds.origin, size: size(px(w), px(pane)) };
                let bottom = Bounds {
                    origin: point(bounds.origin.x, bounds.origin.y + px(second)),
                    size: size(px(w), px(pane)),
                };
                frame.draws.push(draw(top, top, first));
                frame.draws.push(draw(bottom, bottom, last));
                frame.separators.push(Bounds {
                    origin: point(bounds.origin.x, bounds.origin.y + px(pane)),
                    size: size(px(w), px(2.0 * PANE_GAP)),
                });
            }
            _ => frame.draws.push(draw(bounds, bounds, main)),
        }
    }

    /// The ring under the pointer, at the same image point in every pane.
    fn place_rings(&self, frame: &mut Frame) {
        let Some(at) = self.hover.and_then(|p| self.image_point(p)) else { return };
        let image_w = self.view.image_w.max(1) as f32;
        for d in &frame.draws {
            // Logical pixels per image pixel in this pane.
            let scale = f32::from(d.rect.size.width) / image_w;
            let center =
                point(d.rect.origin.x + px(at.0 * scale), d.rect.origin.y + px(at.1 * scale));
            frame.rings.push((center, self.ring.radius * scale, self.ring.hardness, d.clip));
        }
    }

    /// The source's tiles under the inner circle in the canvas pane (last).
    /// Hardness near 0 has no inner circle.
    fn place_preview(&mut self, frame: &mut Frame) {
        let Some(at) = self.hover.and_then(|p| self.image_point(p)) else { return };
        let Some(canvas) = frame.draws.last() else { return };
        let (clip, rect) = (canvas.clip, canvas.rect);
        let device_scale = self.view.device_scale();
        let image = (self.view.image_w, self.view.image_h);
        let Self { secondary, atlas, ring, .. } = self;
        let Some(source) = secondary.as_mut().filter(|s| (s.width, s.height) == image) else {
            return;
        };
        let scale = f32::from(rect.size.width) / image.0.max(1) as f32;
        let inner = if ring.hardness >= 0.98 { 1.0 } else { ring.hardness.sqrt() };
        let radius = ring.radius * inner * scale;
        if radius < 1.0 {
            return;
        }
        let center = point(rect.origin.x + px(at.0 * scale), rect.origin.y + px(at.1 * scale));
        let circle = Bounds {
            origin: point(center.x - px(radius), center.y - px(radius)),
            size: size(px(2.0 * radius), px(2.0 * radius)),
        };
        if !circle.intersects(&clip) {
            return;
        }
        let tiles = source.draws(atlas, rect, circle.intersect(&clip), device_scale);
        frame.preview = Some(Preview { center, radius, clip, tiles });
    }

    /// Paint the frame and register its mouse handlers (re-registered every
    /// frame, GPUI's model, acting on the viewer entity).
    pub(super) fn paint(
        frame: Frame,
        view: &Entity<Viewer>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let divider_color: Hsla = gpui::white();
        let separator_color: Hsla = gpui::opaque_grey(0.5, 0.5);
        let knob_color: Hsla = gpui::rgb(crate::brand::ACCENT_ON_PHOTO).into();

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for d in &frame.draws {
                // One layer per pane, so its tiles batch together.
                window.with_content_mask(Some(ContentMask { bounds: d.clip }), |window| {
                    window.paint_layer(d.clip, |window| {
                        for tile in &d.tiles {
                            paint_tile(tile, window);
                        }
                    })
                });
            }
            if let Some((clip, rect, image)) = &frame.flat {
                window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| {
                    let painted = window.paint_image(
                        *rect,
                        *rect,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                    if let Err(e) = painted {
                        tracing::warn!("the adjust preview wasn't painted: {e}");
                    }
                });
            }
            if let Some(p) = &frame.preview {
                paint_preview(p, window);
            }
            for s in &frame.separators {
                window.paint_quad(fill(*s, separator_color));
            }
            if let Some(d) = frame.divider {
                window.paint_quad(fill(d, divider_color));
                let c = d.center();
                let knob = Bounds {
                    origin: point(c.x - px(14.0), c.y - px(14.0)),
                    size: size(px(28.0), px(28.0)),
                };
                window.paint_quad(quad(
                    knob,
                    14.0,
                    knob_color,
                    0.0,
                    transparent_black(),
                    BorderStyle::default(),
                ));
            }
            for &(center, radius, hardness, clip) in &frame.rings {
                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    // Dark under light so the ring shows on any image; the
                    // inner one marks the hardness.
                    let dark: Hsla = gpui::opaque_grey(0.0, 0.7);
                    let light: Hsla = gpui::opaque_grey(1.0, 0.9);
                    let faint: Hsla = gpui::opaque_grey(1.0, 0.35);
                    paint_ring(window, center, radius + 1.0, dark);
                    paint_ring(window, center, radius, light);
                    if hardness > 0.02 && hardness < 0.98 {
                        paint_ring(window, center, radius * hardness.sqrt(), faint);
                    }
                });
            }
        });

        if frame.dragging {
            window.set_cursor_style(CursorStyle::ClosedHand, &frame.hitbox);
        } else if frame.crosshair {
            window.set_cursor_style(CursorStyle::Crosshair, &frame.hitbox);
        }
        if let Some(h) = &frame.divider_hitbox {
            window.set_cursor_style(CursorStyle::ResizeLeftRight, h);
        }
        Self::listen(&frame, view, window);
    }

    /// Register this frame's mouse handlers on the viewer `view`.
    fn listen(frame: &Frame, view: &Entity<Viewer>, window: &mut Window) {
        let hitbox = frame.hitbox.clone();
        let divider_hitbox = frame.divider_hitbox.clone();
        let held = frame.held.clone();
        let tracking = frame.tracking.clone();
        let pointer = frame.pointer.clone();
        {
            let view = view.clone();
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || !matches!(event.button, MouseButton::Left | MouseButton::Right)
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let on_divider = divider_hitbox.as_ref().is_some_and(|h| h.is_hovered(window));
                view.update(cx, |this, cx| {
                    if event.modifiers.alt
                        && event.button == MouseButton::Left
                        && this.tool == Tool::Paint
                    {
                        this.pick_at(event.position, cx);
                    } else if event.click_count == 2
                        && event.button == MouseButton::Left
                        && !on_divider
                        && this.tool != Tool::Paint
                    {
                        this.double_click(event.position);
                    } else {
                        this.mouse_down(event.position, event.button, on_divider, cx);
                    }
                    cx.notify();
                });
            });
        }
        {
            let view = view.clone();
            let hitbox = hitbox.clone();
            let pointer = pointer.clone();
            let tracking = tracking.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let over = hitbox.is_hovered(window);
                // A key zoom's anchor, set without touching the entity.
                pointer.set(over.then_some(event.position));
                if !(held.get() || tracking.get()) {
                    return;
                }
                view.update(cx, |this, cx| {
                    if this.mouse_move(event.position, over, cx) {
                        cx.notify();
                    }
                });
            });
        }
        {
            let view = view.clone();
            window.on_mouse_event(move |_: &MouseExitEvent, _, _, cx| {
                pointer.set(None);
                // Off the depth map too, out of the window.
                if tracking.get() {
                    view.update(cx, |this, cx| this.point_at_map(None, cx));
                }
            });
        }
        {
            let view = view.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                view.update(cx, |this, cx| {
                    if this.mouse_up(event.button, cx) {
                        cx.notify();
                    }
                });
            });
        }
        {
            let view = view.clone();
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let line_height = window.line_height();
                let zoom = event.modifiers.secondary();
                view.update(cx, |this, cx| {
                    if this.scroll(event.position, event.delta, line_height, zoom) {
                        cx.notify();
                    }
                });
            });
        }
        {
            let view = view.clone();
            window.on_mouse_event(move |event: &PinchEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                view.update(cx, |this, cx| {
                    if this.pinch(event.position, event.delta) {
                        cx.notify();
                    }
                });
            });
        }
    }
}

/// Paint one tile whole, clipped to its core; on failure, the coarser
/// level's tiles under it.
fn paint_tile(tile: &TileDraw, window: &mut Window) {
    window.with_content_mask(Some(ContentMask { bounds: tile.mask }), |window| {
        let painted = window.paint_image(
            tile.image,
            tile.image,
            Corners::default(),
            tile.data.clone(),
            0,
            false,
        );
        if let Err(e) = painted {
            // Atlas allocation can fail (out of memory): log once, not per frame.
            static LOGGED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::error!("paint_image failed: {e}");
            }
            for coarser in &tile.fallback {
                paint_tile(coarser, window);
            }
        }
    });
}

/// The preview's tiles cut to its circle, stacked from horizontal strips
/// since content masks are rectangles.
fn paint_preview(p: &Preview, window: &mut Window) {
    const MAX_STRIPS: f32 = 256.0;
    let r = p.radius;
    let strips = (2.0 * r).ceil().min(MAX_STRIPS);
    let h = 2.0 * r / strips;
    window.with_content_mask(Some(ContentMask { bounds: p.clip }), |window| {
        window.paint_layer(p.clip, |window| {
            for i in 0..strips as usize {
                let top = -r + i as f32 * h;
                let mid = top + 0.5 * h;
                let half = (r * r - mid * mid).max(0.0).sqrt();
                let strip = Bounds::from_corners(
                    point(p.center.x - px(half), p.center.y + px(top)),
                    point(p.center.x + px(half), p.center.y + px(top + h)),
                );
                window.with_content_mask(Some(ContentMask { bounds: strip }), |window| {
                    for tile in p.tiles.iter().filter(|t| t.mask.intersects(&strip)) {
                        paint_tile(tile, window);
                    }
                });
            }
        })
    });
}

/// A 1 px circle outline.
fn paint_ring(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    let r = radius.max(1.0);
    let bounds = Bounds {
        origin: point(center.x - px(r), center.y - px(r)),
        size: size(px(2.0 * r), px(2.0 * r)),
    };
    window.paint_quad(quad(bounds, r, transparent_black(), 1.0, color, BorderStyle::default()));
}

/// The compare images, first (left or top) and last: the source first
/// unless `swapped`.
fn sides<'a, T>(swapped: bool, main: &'a mut T, other: &'a mut T) -> (&'a mut T, &'a mut T) {
    if swapped { (main, other) } else { (other, main) }
}
