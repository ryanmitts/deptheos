# Changelog

## 0.2.0

A new app. Deptheos is rebuilt on a Rust engine that stacks on the GPU through
wgpu (Metal, DirectX 12 or Vulkan), in place of the earlier macOS-only app on
Metal (v0.1).

**Platforms**

- macOS 27 or later on Apple silicon: a signed and notarized `.dmg`.
- Windows 10 or later, 64-bit: an installer or a portable `.zip`. Neither is
  code-signed yet, so SmartScreen may warn on first run.
- Linux, as a preview, on x86-64 and ARM64: `.deb`, `.rpm` and a tarball, for
  any distribution with glibc 2.35 or newer and a Vulkan driver.
- The iPhone and iPad app is coming to the App Store.

**Stacking**

- Three methods: pyramid (the default, for hairs and fine crossing detail),
  depth map (smooth surfaces and plain backgrounds) and wavelet.
- Two aligners for handheld and rail stacks: ECC (a full affine transform, the
  default) and grid search. A frame that doesn't align or can't be read is left
  out, and the result can be cropped to what every frame covers.
- JPEG, PNG and TIFF frames, and camera RAW and DNG, decoded by LibRaw or by
  Adobe DNG Converter when it is installed; LibRaw stacks use Adobe's camera
  profiles when they are installed.

**Export**

- Linear DNG from RAW and DNG stacks, still editable in a raw editor; TIFF,
  PNG and JPEG from any stack, 8-bit saves dithered.
- The reference frame's camera, lens, exposure and capture time are kept, with
  the stack's method and source files. GPS, serial numbers, the owner's name
  and maker notes are never written.

**The app**

- A Library that keeps every render of every stack; a deleted one goes to
  Recently Deleted.
- A viewer with a split view to compare two renders, or a render and a source
  frame, and a focus rail through the render's frames.
- Retouch: paint from any frame or another render.
- Batch: a card's worth of stacks in one run, split by capture time.
- Two sample stacks built in, Hornet and Circuit Board, under Help ▸ Try a
  Sample Stack.

**Command line**

- `deptheos`, installed with the app: `pyramid`, `depth` and `wavelet` stacks,
  `batch`, `align`, `sequence grow|turntable` for numbered frames to make a
  video from, and `bench` to time stacks on your machine.

**Privacy**

- Frames and renders stay on your computer. The desktop app's only request is
  the update check at startup, which sends the app's version and can be turned
  off in Settings ▸ General. The command line makes none.
