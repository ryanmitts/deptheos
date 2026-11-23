# ADR-120: The stackers are tuned by eye on real stacks, keep their own kernels, and treat every edge alike

## Status

Accepted.

## Context

Focus-stacking quality has no single scalar metric. Synthetic benchmarks with a known sharp picture and high-frequency energy measures catch regressions, but a sharpness proxy rewards sensor grain as if it were detail, and several changes that scored better looked worse on real stacks.

The three stackers (Pyramid, Depth, Wavelet; ADR-111) each make choices a later change could undo without knowing why: the constants they were tuned with, the filter kernels of their pyramids, how the Depth stacker smooths its depth map, and how each handles the edges of a frame and of the picture. The algorithms themselves are in `docs/algorithms/`.

## Decision

### Choices are judged by eye on full-size renders of real stacks

Selection and blending are tuned by looking at full-size renders of real stacks, not by a score. `script/compare/` makes that practical over many candidates: it shows two renders with their sides shuffled, in a flip or a split viewer, and appends each vote (one, the other or a tie) to `votes.jsonl` before saying which was which. It is a convenience for judging, not a gate a change must pass.

A change with several parameters is checked on held-out stacks of varying frame counts (200-frame stacks among them) and resolutions, not only on the stacks it was tuned on. A change that alters the character of a method (making Depth behave like Pyramid, or fitting defaults to a few stacks) is not adopted even if it looks better on a few stacks. The tuned constants this rests on are Pyramid's focus prior exponent (`PRIOR_EXPONENT`, 0.75), Depth's focus-metric blur scale (`RADIUS_BLUR_SCALE`, 2.4), Wavelet's center-weighted 3×3 selection window (weights 4/2/1), and Depth's general-purpose defaults, Radius 8, Smoothing 4 and Blend Range 3 (`DepthConfig`).

### Each stacker keeps its own pyramid kernel

Pyramid uses Burt and Adelson's 5-tap binomial Gaussian, `[1, 4, 6, 4, 1] / 16`, with the parity-dependent EXPAND (`shaders/laplacian.wgsl`). Depth uses a σ = 0.666 Gaussian that drops the taps outside the image and renormalizes the rest (`shaders/resample.wgsl`). The two share their pyramid plumbing in `accum.rs`: one set of level sizes, one `vec4f` storage-buffer accumulator layout, and one finish step for the collapse.

### The depth map is smoothed by a confidence-guided IIR blur on a half-size grid

A winner-take-all depth map is noisy wherever no frame is clearly sharp, and it must be smoothed without moving the depths that are confident. The map is built and smoothed on a grid at most half the frame size, its long side capped at 6,000 px. The smoother is a first-order exponential (IIR) filter run forward and backward along a line, the two averaged, along rows and then columns, the whole applied twice. Each pixel's coefficient interpolates between a weak and a strong setting by the pixel's confidence alone, with no gradient or edge term. The coefficients are quantized so that a setting blurs the same on every GPU, and each sweep is its own dispatch. Each sweep starts from the input at its own end, so both ends of a line are treated alike (`smoothing_treats_both_ends_alike`).

### The wavelet's edges see their own picture

The wavelet transform's boundary is half-sample symmetric at every edge (`wavelet.wgsl`), so a frame alone comes back exactly (`the_transform_reconstructs_a_frame`). The coarsest band is a two-class sum, as the pyramid stacker's coarsest level is: per coefficient, the mean of the frames whose footprint covers it, or of every frame where none does. Off a frame's footprint, its selection energy in the detail bands is scaled down, so a frame with data wins there.

## Alternatives considered

### Decide by a numerical metric alone

Metrics disagreed with what the renders showed: a coherent pyramid score halved fine-scale energy (mostly background noise) and still lost, and stronger depth smoothing scored better while only tying.

### A thin-structure override in Depth

It won on the stacks its thresholds were tuned on and lost on held-out ones, with ragged patches and blotches, at about 300 MB more at 48 MP. Thin crossing structures are what Pyramid is for.

### One method from the depth map and the pyramid's finest band

It doubled the finest band's noise in out-of-focus areas on a 200-frame stack and needed about 600 MB more GPU memory at 45 MP. Depth and Pyramid stay separate methods, chosen per stack.

### One kernel for both stackers, or a generic pyramid type over both

Pyramid on `resample.wgsl` lost sharpness and ran slower; Depth on `laplacian.wgsl` would lose the dropped-tap edges and the exact alignment between the depth-map pyramid and the blend's levels. A generic type would save little code for one more layer.

### Smooth at full resolution with an edge-aware recursive filter

More GPU memory, a much wider post-blur, a risk of Windows' GPU watchdog (TDR) on long serial sweeps, and it over-smoothed across the subject.

### A circular boundary in the wavelet transform

The left and top edges saw the far side of the picture wrapped in and took the blurred frame where the right and bottom didn't (`every_edge_takes_the_sharp_frame`).

### Weighting frames without data in the coarsest band

The small weight adds up over a long stack and pulls the band toward the edge smear (`frames_without_data_leave_the_coarsest_band_alone`).

## Consequences

- Each key tuning constant traces to a judgment described here: `PRIOR_EXPONENT`, `RADIUS_BLUR_SCALE`, the wavelet's 4/2/1 window and the Depth defaults.
- `laplacian.wgsl` and `resample.wgsl` stay separate in `crates/deptheos_core/src/shaders/`, on purpose.
- An IIR sweep costs the same at any blur radius, and the capped half-size grid keeps the serial sweeps short enough for drivers' watchdogs (ADR-100).
- Because the smoothing follows confidence rather than image edges, a confident foreground structure such as a hair spreads its depth slightly into the unconfident background around it, and a featureless area wider than the blur's reach keeps the patch-wise pick.
- Repeat runs give the same depth map (`depth_map_is_deterministic`); the golden tests hold each stacker's output to a reference (ADR-113).

## Code

- `crates/deptheos_core/src/pyramid/mod.rs`, `crates/deptheos_core/src/pyramid/pyramid.wgsl`.
- `crates/deptheos_core/src/depth/mod.rs`, `crates/deptheos_core/src/depth/curves.rs`, `crates/deptheos_core/src/depth/depth.wgsl`.
- `crates/deptheos_core/src/wavelet/wavelet.wgsl`.
- `crates/deptheos_core/src/shaders/laplacian.wgsl`, `crates/deptheos_core/src/shaders/resample.wgsl`, `crates/deptheos_core/src/accum.rs`.
- `script/compare/`.
- `docs/algorithms/pyramid.md`, `docs/algorithms/depth-map.md`, `docs/algorithms/wavelet.md`.
