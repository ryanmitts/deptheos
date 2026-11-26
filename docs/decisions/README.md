# Decisions

Why Deptheos is built the way it is. Each record (an ADR) states one decision,
the alternatives considered, and its trade-offs.
`docs/algorithms/` describes how the algorithms work; these records explain why.

## Writing a new one

Write an ADR when a choice would be expensive to reverse or a contributor would
be likely to question it: a dependency, where work runs, a file format, an
algorithm chosen over another, something built in-house where a library exists,
or an approach that was tried and removed. Tuning constants belong in
`docs/algorithms/` and the code.

Copy the sections of an existing record (Status, Context, Decision,
Alternatives considered, Consequences, Code), take the next free number in its
area's block (100–109, 110–119, 120–129, 130–139, 140–159, 160–179), and add it
to the index below. Keep it to about 600 to 1,000 words: the decision and why,
not an inventory of the code. Code lists files, not symbols.

Status is "Accepted"; it holds no test status or plans. When a decision is
reversed, write the new record and delete the old one. A Status, like the rest
of a record, cites only records in this folder.

## Platform and front ends

- [100](100-the-engine-is-rust-on-a-headless-wgpu-device.md) The engine is Rust on one headless wgpu device; front ends get CPU pixels
- [101](101-the-front-ends-are-shells-over-deptheos-app.md) The front ends are shells over `deptheos_app`: the desktop on GPUI, iOS in SwiftUI over UniFFI
- [102](102-releases-native-packages-on-tested-platforms.md) Releases are native packages for the platforms the owner tests on; the app points to updates and carries the CLI
- [103](103-the-command-line.md) The command line: a command per method, paths on stdout, refusals before any work, never over an input

## GPU pipeline and alignment

- [110](110-gpu-data-and-shaders.md) 32-bit float data in storage buffers, rounded once to 16 bits, from WGSL modules composed by naga_oil
- [111](111-frames-streamed-memory-planned-gpu-failures-are-errors.md) Stackers take frames one at a time, a stack's memory is planned first, and GPU failures are errors
- [112](112-alignment-on-the-cpu-each-frame-to-the-one-before.md) Alignment runs on the CPU, with two aligners that each align a frame to the one before
- [113](113-golden-references-and-where-the-tests-run.md) Golden references hold each stacker's output within a GPU's rounding; the Rust tests run on the developer's GPU and on CI's software GPUs

## Stacking algorithms

- [120](120-the-stackers-are-tuned-by-eye-and-keep-their-own-kernels.md) The stackers are tuned by eye on real stacks, keep their own kernels, and treat every edge alike
- [121](121-a-frame-that-fails-is-left-out-and-frames-are-not-scored.md) A frame that fails is left out, a few at most; a stack shows where each frame went, not a score for it

## RAW, DNG and color

- [130](130-raw-stays-linear-and-renders-on-the-gpu.md) RAW stacks stay linear camera RGB, their color rendered on the GPU from the DNG SDK's parameters
- [131](131-raw-and-dng-decoders.md) Camera RAW through LibRaw, Adobe DNG Converter or Apple's decoder; every DNG through the Adobe DNG SDK
- [132](132-standard-images-one-handover-one-input-stage.md) Every decoder of standard images hands over the same thing; one input stage makes it a frame in the stack's color space
- [133](133-exports-encoders-metadata-and-dither.md) Exports are encoded by Rust crates with metadata we write ourselves, and dithered at 8 bits
- [134](134-native-libraries-and-the-license.md) The native libraries are vendored and built by our build scripts; GPL-3.0 with permissions for Adobe's SDKs and app stores
- [135](135-every-draw-takes-a-look.md) Every draw of a stack takes a look (the stack, its adjustments, an output space), resolved once and drawn by one render function

## The model, the workflow and saved data

- [140](140-the-library-is-a-store-of-renders-one-apps-at-a-time.md) The Library is a store of immutable renders, one app's at a time; a delete goes to Recently Deleted
- [141](141-one-persistence-layer.md) One persistence layer: the Library's format checked as it opens, atomic writes, damaged files set aside
- [142](142-stacks-renders-and-the-working-version.md) Stacks of renders, each with one working version; a past render is shown, never opened; the workflow rules
- [143](143-lifecycles-current-requests-and-the-engines-threads.md) One enum per lifecycle, results applied only while current, and the engine's threads
- [144](144-rules-both-apps-share-are-pure-functions-in-the-engine.md) The rules both apps share are pure functions in the engine: the batch run, the Library's tiles, thumbnails, adding frames
- [145](145-frames-are-a-root-and-a-path.md) A frame is a root and a relative path; iOS reads folders in place and copies only where a reference would break
- [146](146-a-folder-is-read-as-shots-in-name-order.md) A folder is read as shots in name order; capture time only splits it; list thumbnails from embedded previews
- [147](147-renders-line-up-and-sequences-replay-through-recorded-geometry.md) Renders line up through recorded transforms; grow and turntable sequences replay pyramid snapshots
- [148](148-paint-is-a-cpu-tile-canvas-saved-as-one-sqlite-file.md) Paint is a CPU tile canvas with session undo, saved as one SQLite file per render
- [149](149-the-viewer.md) The viewer: one view reducer, a focus rail through the render's frames, the depth map as the frame map, Contribution as one light
- [150](150-shared-preference-definitions.md) Preferences both apps offer are defined in `deptheos_app`; each front end keeps its own storage
- [151](151-the-apps-carry-a-sample-stack.md) Both apps carry two sample stacks, each copied into the user's storage and opened as a new stack that waits for Stack

## Website, iOS, logs, localization and tools

- [160](160-website-and-design-tokens.md) The website is static Astro pages with the engine's numbers and the app's renders; one tokens file for both apps
- [161](161-the-apps-words.md) One page of terms governs every string; English only, in Fluent files, a String Catalog and the CLI's code
- [162](162-ios-camera.md) The iOS camera shoots stacks itself: focus-locked bursts, a layout from the screen's shape, its own focus peaking
- [163](163-ios-stacking-and-retouch-screens.md) iOS Stacking and Retouch: the photo full bleed, one Frames | Renders switch, one sheet at a time, one Retouch deck
- [164](164-ios-pictures-memory-and-device-floor.md) iOS shows the full image through one tiled view, holds only what is on screen, and installs only on the performance tier
- [165](165-ios-background-gpu-work.md) iOS stacks go on in the background under BGContinuedProcessingTask
- [166](166-diagnostic-log-file.md) Every front end logs through tracing; the apps keep a file users send with bug reports
- [167](167-stackrig-raw-ptp-and-ble.md) stackrig talks raw PTP and BLE, with no dependencies
- [168](168-website-hosting.md) The pages are a Cloudflare Worker's static assets, the media is in an R2 bucket under content hashes; deployed by hand
- [169](169-ios-layout-reads-flow-one-way.md) On iOS, a measured value never sizes or moves the view it was measured from
- [170](170-ios-backup-keeps-the-library-but-recently-deleted.md) iOS backs up the Library, all but Recently Deleted

## Earlier records

Records 001–081 were folded into the current ones; git history holds them.
