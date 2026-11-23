# Deptheos

<p align="center">
<img src="https://images.deptheos.com/deptheos_logo.jpg" alt="The Deptheos logo" width="150">
</p>

> Deptheos is a free, open-source focus stacker for macro photography. The stacking runs on the GPU, so a stack of 200 frames at 45 MP is aligned and stacked in seconds. Three methods (pyramid, depth map and wavelet), RAW in, linear DNG out, a split view to compare renders, retouching, batch with auto sort, and a Library that keeps every render.

<img src="https://images.deptheos.com/hornet_2_small.jpg" alt="A hornet, stacked with Deptheos" width="800" />

**[Download the latest release](https://github.com/ryanmitts/deptheos/releases/latest)** · [deptheos.com](https://deptheos.com) · [Docs](https://deptheos.com/docs/) · [Gallery](https://deptheos.com/gallery/)

## What it does

- **Three stacking methods**: pyramid (the default, for hairs and fine crossing detail), depth map (smooth surfaces and plain backgrounds) and wavelet. [Choosing a method](https://deptheos.com/docs/methods/)
- **Alignment** for handheld and rail stacks, with two aligners. [Aligning the frames](https://deptheos.com/docs/alignment/)
- **RAW and DNG in, linear DNG out**, still editable in a raw editor; or TIFF, PNG and JPEG. [RAW, DNG and color](https://deptheos.com/docs/raw/)
- **Compare** two renders, or a render and a source frame, under a line you drag; **retouch** from any frame or another render.
- **Batch**: a card's worth of stacks in one run, split by capture time.
- **A command line**, `deptheos`, that does the same from a terminal. [The command line](https://deptheos.com/docs/cli/)

The website has [every feature](https://deptheos.com/features/), [stacking times](https://deptheos.com/#speed) on named hardware and the docs. New here? Start with [your first stack](https://deptheos.com/docs/getting-started/).

## Download

Installers are on the [Releases](https://github.com/ryanmitts/deptheos/releases/latest) page; [Installing](https://deptheos.com/docs/install/) has the steps for each system.

| Platform | Needs | Files |
|---|---|---|
| macOS | macOS 27 or later, Apple silicon | a signed and notarized `.dmg` |
| Windows | Windows 10 or later, 64-bit | an installer (not code-signed yet, so SmartScreen warns), or a portable `.zip` |
| Linux | x86-64 or ARM64, a Vulkan driver | `.deb`, `.rpm` or a tarball |
| iPhone and iPad | | coming soon, on the App Store |

Stacking runs on the GPU (Metal, DirectX 12 or Vulkan; integrated graphics will do), and 16 GB of memory is recommended.

Your frames and renders stay on your computer. The desktop app makes one request over the network, the update check: at startup it asks GitHub for the latest release, sending only the app's version. Turn it off under Settings ▸ General ▸ Check for updates. The command line and the iPhone and iPad app make none. Nothing else is collected and nothing tracks you: the [privacy policy](https://deptheos.com/privacy/) says exactly what is sent, and [Support](https://deptheos.com/support/) how to send diagnostics when you choose to.

## Building

The workspace is Rust with a few C++ libraries: LibRaw (a submodule) and the Adobe DNG SDK (vendored). `just check` needs [Rust](https://rustup.rs) 1.93 or newer (`rust-version` in `Cargo.toml`), [just](https://just.systems) and [Node](https://nodejs.org) 22 for the website's tests.

```sh
git submodule update --init
(cd web && npm install)                # the website's packages, which `just check` tests
cargo run --release -p deptheos_gpui   # the desktop app
just check                             # the fast tests; `just` lists every recipe
```

How the crates fit together and where a stack runs: [docs/architecture.md](docs/architecture.md).

- macOS: Xcode 27 or newer, whose SDK the macOS 27 floor needs (ADR-102); the full Xcode, not only the command line tools.
- Windows: Visual Studio's C++ tools; see [docs/windows.md](docs/windows.md).
- Linux: a C++ toolchain, `pkg-config` and the development headers GPUI links against. On Ubuntu or Debian:

  ```sh
  sudo apt install build-essential pkg-config cmake clang \
    libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev libasound2-dev \
    libssl-dev libzstd-dev
  ```

  These are only for building. To run the app, you also need a Vulkan driver (`mesa-vulkan-drivers`, or your GPU vendor's). In a VM, turn on the virtual GPU's acceleration (in UTM, virtio-gpu with GPU acceleration on), or the app falls back to Mesa's CPU renderer.
- iOS: the app links the Rust engine as an XCFramework, which Xcode does not rebuild. Add the Rust targets once (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`). Run `just ios-engine` (a debug build of the XCFramework; `ios/build-xcframework.sh` alone makes a release one) first and again after every Rust change, then open `ios/DeptheosMobile.xcodeproj`, or `just ios-build` to build it by scheme from the command line.

  The app opens in the Simulator but can't stack there: the Simulator's GPU lacks a feature the engine needs. Stack on a device. To sign it for a device, set your own Apple team in place of `T5PCWU6J56` in `ios/DeptheosMobile.xcodeproj` and `ios/ExportOptions.plist`.
- Release builds and installers: [docs/releasing.md](docs/releasing.md).
- The website: `cd web && npm install && npm run dev`; the rest, including deploying and retaking the benchmarks, is in [web/README.md](web/README.md).

## The apps' text

The words to use are in [docs/terms-and-usage.md](docs/terms-and-usage.md). The strings: the shared layer's in `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`, the desktop's in `crates/deptheos_gpui/i18n/en-US/deptheos_gpui.ftl`, iOS's in `ios/DeptheosMobile/Localizable.xcstrings` (brought up to date by `xcodebuild -exportLocalizations`, as the terms doc says); the command line's and stackrig's in their source.

## Command line

`deptheos` comes with the app and stacks from a terminal:

```sh
deptheos pyramid -o out.dng frames/*.CR3
deptheos depth -o out.tif frames/*.jpg
deptheos batch pyramid --split auto -o renders/ card/
deptheos bench frames/   # time whole stacks on your machine
```

`deptheos --help` lists the commands; [the command line](https://deptheos.com/docs/cli/) has where it is installed on each system and the options. From the source, `cargo run --release -p deptheos_cli -- <command> …` does the same.

## Sample stacks

Spider, 46 frames: [JPEG (250 MB)](https://images.deptheos.com/spider1-jpeg.zip) · [CR3 (711 MB)](https://images.deptheos.com/spider1-cr3.zip)

Hornet, 66 frames, handheld: [JPEG (264 MB)](https://images.deptheos.com/samples/hornet-jpeg.zip) · [CR3 (1.1 GB)](https://images.deptheos.com/samples/hornet-cr3.zip)

Circuit board, 200 frames, on a tripod: [JPEG (412 MB)](https://images.deptheos.com/samples/pcb-jpeg.zip) · [CR3 (3.1 GB)](https://images.deptheos.com/samples/pcb-cr3.zip)

## References

How each method works, step by step, is in [docs/algorithms](docs/algorithms/README.md). Each page lists the work it follows or is related to, and says for which part.

- Pyramid: Burt, P. J. & Adelson, E. H. (1983). The Laplacian Pyramid as a Compact Image Code. *IEEE Transactions on Communications* 31(4), 532–540. doi:10.1109/TCOM.1983.1095851. The pyramid is theirs; the focus weights are described in [docs/algorithms/pyramid.md](docs/algorithms/pyramid.md).
- Depth map: Nayar, S. K. & Nakagawa, Y. (1994). Shape from Focus. *IEEE Transactions on Pattern Analysis and Machine Intelligence* 16(8), 824–831. The map takes, at each pixel, the frame whose focus measure is highest, as their coarse depth map does.
- Wavelet: Forster, B., Van De Ville, D., Berent, J., Sage, D. & Unser, M. (2004). Complex wavelets for extended depth-of-field: a new method for the fusion of multichannel microscopy images. *Microscopy Research and Technique* 65, 33–42. doi:10.1002/jemt.20092. Deptheos's wavelet method is adapted from Petteri Aimonen's [focus-stack](https://github.com/PetteriAimonen/focus-stack).
- ECC alignment: Evangelidis, G. D. & Psarakis, E. Z. (2008). Parametric Image Alignment Using Enhanced Correlation Coefficient Maximization. *IEEE Transactions on Pattern Analysis and Machine Intelligence* 30(10), 1858–1865. doi:10.1109/TPAMI.2008.113. Deptheos's ECC aligner is derived from OpenCV's [`findTransformECC`](https://github.com/opencv/opencv/blob/4.x/modules/video/src/ecc.cpp).
- Grid alignment: Jain, J. R. & Jain, A. K. (1981). Displacement Measurement and Its Application in Interframe Image Coding. *IEEE Transactions on Communications* 29(12), 1799–1808. The grid search's step rule is a per-axis variant of their logarithmic search's.
- Szeliski, R. (2006). Image Alignment and Stitching: A Tutorial. *Foundations and Trends in Computer Graphics and Vision* 2(1), 1–104. doi:10.1561/0600000009

## License

GPL-3.0, with additional permissions to combine Deptheos with the Adobe DNG SDK, the XMP Toolkit and LibRaw and to distribute it through app stores ([LICENSE.md](LICENSE.md)). Contributions are taken under the same terms ([CONTRIBUTING.md](CONTRIBUTING.md)). The name and logo are not covered by the license: forks are welcome under their own name ([TRADEMARKS.md](TRADEMARKS.md)). The photographs (the sample stacks, the website's pictures and the test images) are all rights reserved and not under the GPL ([LICENSE.md](LICENSE.md)).

Built with [wgpu](https://wgpu.rs), [LibRaw](https://www.libraw.org) and the [Adobe DNG SDK](https://helpx.adobe.com/camera-raw/digital-negative.html). Every bundled library and its license is listed in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt); regenerate it with `just notices` after changing a dependency.
