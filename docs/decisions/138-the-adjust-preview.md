# ADR-138: The drag preview is one flat picture a tick, from the reduced copy at fit and from the full-size source past half zoom, and it settles on screen before the whole picture redraws

## Status

Accepted.

## Context

While a slider moves, the picture has to follow each value within a frame or two. At 45 megapixels a full redraw with Highlights, Shadows or Clarity takes 590 ms on an M5 Max, most of it the local tone's per-pixel values worked out on the CPU (ADR-114). The viewer's pyramid of tiles is built from a full redraw, and rebuilding it each tick costs more than the draw. So a tick needs a picture of its own, at the size that is on screen, made from copies kept for it, and it must end a drag on the picture that settles, without a jump.

The budgets at 45 MP: a tick at fit in 16 ms; at 100% in 16 ms, or 33 ms with Texture or Sharpening set (`Tier::budget`); and the settled picture on screen 250 ms after a drag ends (`SETTLE_BUDGET`).

## Decision

### Two tiers

`GpuCopies::draw_preview` (`crates/deptheos_app/src/adjust/copies.rs`) draws one tick through the look Adjust draws the screen with (`Look::adjusting`, always the adjusted path, so every tick and the picture it settles on draw alike). `Tier::of` (`crates/deptheos_app/src/adjust/preview.rs`) picks how:

- **Fit, up to 50% zoom: the whole picture at the size shown, from the reduced copy.** The size is the picture's on screen, at most 2048 on the long side. The reduced copy (ADR-114) is resampled to it once for each size, and drawn in one fused pass.
- **Region, past 50% zoom: the part on screen at full size, from the full-size source**, every slider, as one flat picture rather than tiles. It is that window of the full draw bit for bit, since a region draws as the whole picture does (ADR-135).

While the device runs hot (iOS's serious or critical thermal state, `AdjustSession::set_hot`), a tick at fit is drawn at most `HOT_FIT_LONG` (1024) on its long side, a quarter of the pixels, and stretched on screen; past 50% it is unchanged, and the settled picture is drawn as always.

Each tick is read back into one flat picture (`FlatPicture`): 8-bit pixels in the display's channel order, naming the stored pixels they cover and their color space, sRGB or a standard stack's own profile with Color Management off. Its buffer comes from a `FlatSlot` the app hands each replaced picture back to; on iOS the app reads it in place (ADR-139). A tick allocates none once the slot holds the two it needs, the picture on screen and the one being drawn.

### The fit tier keeps the settled picture's level

The viewer's levels take the mean of display values, and a mean of linear values is brighter on fine detail: dark lines a pixel or two wide, reduced four times, came out 16 codes brighter. So the reduced copy and its fit copy take each texel's mean in values as they show, and turn it linear again (`reduce.wgsl`). A standard stack's values go through sRGB's curve, which is close to how they show. A linear stack's go through its own response: a gray ramp from black to the camera's white, drawn once through its constants at the copy's making (`Encoding`, `crates/deptheos_render/src/dng_render/reduce.rs`). On the fine-detail scene, a tick from neutral to every slider at once is 3.5 codes from the settled picture on average. Its mean level is off by 0.5 codes on a standard stack and 2.4 on a linear one.

### The local tone in a tick

- **At fit, the planes are the full-size pixels'.** The local tone's per-pixel values of the whole frame are gathered onto a grid of the fit tier's largest size as they are worked out (`GridPlanes`, `crates/deptheos_render/src/dng_render/draw.rs`). Each grid texel is the plain mean of what it covers. The grid is resampled to the size shown and read in place of values made from the copy. A full redraw gathers the grid as it draws, so the drag after it starts from that draw's planes. Planes made from the reduced copy itself would differ from the export's.
- **Texture and Sharpening are left out at fit** (`Constants::without_fine_detail`): a reduced picture misrepresents a filter a few pixels wide. They show in the region tier, and in the settled full picture at every zoom, where the viewer's levels shrink them as they shrink the rest.
- **Past half zoom, the region's values are kept** for the next tick at that region, so a drag on one slider draws only the pass.
- **A tick takes kept planes of the same sliders** (`PlanesKey::same_sliders`), even when the white or the profile has moved since. Making them anew takes about a second at 45 MP, which a white balance drag would pay every tick. The settled picture makes them for its value (`Draft::Settled`).

### The vignette stays put

A draw that isn't of the stored picture at full size takes the reframe's rectangle in its own pixels. The fit tier scales it to the copy's size (`Constants::scaled`). A region is drawn at frame coordinates, so the rectangle needs no change.

### The histogram and the thumbnail are small draws of the reduced copy

- **The histogram is counted from a draw of the whole picture 512 on its long side** (`GpuCopies::histogram`, `crates/deptheos_app/src/adjust/histogram.rs`), from the reduced copy, under the crop, so it is the same at any zoom or pan and in the engine for both apps. Its bins are 8-bit display values per channel and Rec. 709 luma. It keeps the settled picture's level: on the fine-detail scene its channels' means are within 2 codes of the full draw's under the crop, and on a plain picture with a vignette their distributions too. The small draw has copies of its own beside the fit tier's, so a histogram each tick doesn't make the fit tier resample. A tick's histogram takes 2.4 ms at 45 MP (2.6 with the local tone).
- **The clipping flags come from every pixel.** The small draw counts the pixels at 8-bit 0 and 255 per channel, but its means hide a highlight a pixel wide. When a drag ends, the session counts them again on the GPU over the settled full redraw under the crop (`histogram::clipping`, `DngRenderPipeline::clipped`, `crates/deptheos_render/src/dng_render/clip.rs`), with the same thresholds, in 1.6 to 3 ms at 45 MP and nothing read back but six counts.
- **A thumbnail is drawn the same way** (`GpuCopies::thumbnail`): the whole picture at the size that gives the reframe's window `THUMBNAIL_SIDE` on its long side, from the reduced copy without Texture and Sharpening, its vignette on the window scaled to the draw; the window cut, flipped and turned on the CPU. A window too small in the reduced copy is drawn at full size from the source, every slider, and reduced on the CPU. It is in sRGB, as `thumb.jpg` is (ADR-140).

### The settle

A full redraw with the local tone misses 250 ms (503 ms at 45 MP with every slider, `just adjust-bench`), so the picture on screen settles first and the rest follows. When a drag ends, `AdjustSession` draws the view on screen as `Draft::Settled` and hands it over at once (`Update::OnScreen`), then draws the full picture, counts its clipping over every pixel and hands that over (`Update::Settled`), which gives the viewer its levels for other zooms and pans. Past 50% the view on screen is that window of the full picture, bit for bit; at fit it is drawn from the reduced copy without Texture and Sharpening, which then come with the full picture. The view on screen is shown only while its settle is still current (`Machine::is_settling`). Its draw makes the local tone's planes for this value when they aren't (a white, or a local slider newly set), about half a second at 45 MP; exposure, the curve, color and a set local slider's strength keep them (`GpuCopies::is_settled`). Without the local tone, the full redraw takes 5 ms.

The settle on screen with every slider set and its planes kept, median and 95th percentile at 45 MP: 4.8 / 14.8 ms at fit and 6.6 / 75.2 ms at 100%, both within 250 ms (`just adjust-bench`).

Measured at 45 MP (8256 × 5504) on an M5 Max, a synthetic standard stack, 30 ticks each, median and 95th percentile, readback included:

| Tick | ms |
|---|---|
| Fit, per-pixel sliders | 2.6 / 2.9 |
| Fit, Highlights, Shadows and Clarity | 2.6 / 2.8 |
| 100%, per-pixel sliders | 2.8 / 3.1 |
| 100%, Highlights, Shadows and Clarity | 3.0 / 4.6 |
| 100%, with Texture and Sharpening | 4.3 / 4.7 |

A full redraw takes 3.5 ms (p95 17) with the per-pixel sliders alone, and 590 ms (p95 670) with the local tone, gathering the grid. The first tick with the local tone makes the planes and the grid, 1.0 s, and the first region tick with Texture newly set makes its planes, 0.55 s.

## Alternatives considered

- **Rebuild the viewer's tiles each tick.** Cutting tiles and halving levels costs more than the draw, and a tick needs one picture at the size shown.
- **Draw every tick at full size and shrink it.** At fit that is a 45 MP draw a tick, with the local tone's per-pixel values on the CPU, 590 ms.
- **A reduced copy averaged in linear light.** It is the physically right mean, but the viewer shows the mean of display values, so a drag would end with a jump on fine detail.
- **Planes made from the reduced copy**, as lightcraft's draft preview does. They are cheaper, but the base and Clarity's filter would differ from the export's, a pop at every drag's end.
- **Draw only the tiles on screen at the end of every drag.** Where the planes hold, the last tick is already that picture.

## Consequences

- A tick's level is held to the settled picture's by a test on a fine-detail scene, not proved: a linear stack's copy takes its means through its response at the copy's making, so a strong exposure drag ends with a small step. The bound is in `preview_tests.rs`.
- A white balance drag with the local tone set shows the planes of the white it started from until it settles.
- The session must hand each flat picture back to its slot once it is replaced on screen, or ticks allocate again.
- A tick's histogram narrows on fine detail, which the small draw averages, as the fit tier does; its level holds.
- A thumbnail leaves out Texture and Sharpening, which at its size show little.

## Code

- `crates/deptheos_app/src/adjust/preview.rs`
- `crates/deptheos_app/src/adjust/copies.rs`
- `crates/deptheos_app/src/adjust/preview_tests.rs`
- `crates/deptheos_app/src/adjust/histogram.rs`
- `crates/deptheos_render/src/dng_render/clip.rs`, `crates/deptheos_render/src/dng_render/clip.wgsl`
- `crates/deptheos_render/src/dng_render/reduce.rs`
- `crates/deptheos_render/src/dng_render/reduce.wgsl`
- `crates/deptheos_render/src/dng_render/draw.rs`
- `crates/deptheos_app/src/engine/look.rs`
