# ADR-134: The native libraries are vendored and built by our build scripts, and Deptheos is GPL-3.0 with additional permissions for Adobe's SDKs, LibRaw and app stores; the name is not

## Status

Accepted.

## Context

The engine needs LibRaw and the Adobe DNG SDK, with the SDK's companion libraries: XMP (with expat), `libjxl` (with brotli, highway, skcms) and IJG `libjpeg` 9c, about 700 C and C++ translation units. They must compile cleanly on macOS, iOS, Windows and Linux from a single `cargo build`, and the DNG SDK needs IJG `libjpeg` 9c exactly, not `libjpeg-turbo`.

Deptheos is licensed under GPL-3.0 so that modifications and derivative works remain open source. It links the Adobe DNG SDK and XMP Toolkit (ADR-131), whose license includes indemnity and governing-law clauses that GPL-3.0 treats as "further restrictions" unless explicitly permitted by the copyright holder under section 7. In addition, distributing the iOS app through Apple's App Store requires a section 7 permission because app-store terms impose distribution conditions outside standard GPL-3.0. LibRaw offers two licenses, LGPL-2.1 and CDDL-1.0. The LGPL asks that a user can relink the program with a changed LibRaw and that nothing further restricts them, which an App Store binary that links LibRaw statically can't honor, and a GPL-3.0 permission from Deptheos's authors can't waive LibRaw's own terms. Finally, the release bundles LibRaw, `libjxl`, IJG `libjpeg`, `expat`, and Rust crates, each requiring attribution notices.

## Decision

### Vendored sources

- **The Adobe DNG SDK 1.7.1 package (build 2768)**, with its bundled XMP, `libjxl` and `libjpeg`, lives in `native/vendor/`, unmodified but pruned of files the build never compiles or includes, `XMPFiles` among them (`qDNGXMPFiles=0`), as `native/vendor/PRUNED.md` records. XMP's expat is replaced by the `lib` sources of the expat 2.10.0 release, unchanged, for its security fixes.
- **LibRaw is a git submodule**, `native/vendor/LibRaw`, over HTTPS, pointing to a LibRaw 0.23.0 fork (the version `raw_c_api.cpp` asserts) carrying a commit that enables multi-threaded decoding on Apple platforms through Grand Central Dispatch. LibRaw is always built in: building the engine needs the submodule.

### Built by `build.rs`

- **The `cc` crate compiles everything:** `crates/deptheos_raw/build.rs` builds LibRaw and `crates/deptheos_dng/build.rs` builds the DNG SDK stack, with per-target flags for macOS, iOS, Windows (MSVC) and Linux (GCC). LibRaw uses OpenMP with MSVC (`/openmp`) and with GCC on Linux (`-fopenmp`, GCC's libgomp), and GCD on Apple; a Clang build on Linux decodes on one core. No `bindgen` is used, so building needs no `libclang`. Both scripts take their compiler settings from one place, `crates/native_build/src/flags.rs`: vendored code builds without warnings, and our own C++ (the RAW and DNG bridges) with warnings on under GCC and Clang, the vendored headers taken as system ones; both define `NDEBUG` in every profile.
- **C++ exceptions cross libjpeg's C frames, so they are built for it.** The DNG SDK's libjpeg error handler throws out of libjpeg's calls (a lossy JPEG DNG's tiles, a written DNG's previews). libjpeg is compiled with unwind tables (`-fexceptions`; MSVC makes them on every 64-bit target), and MSVC builds every native library, LibRaw's included, with `/EHs` rather than `/EHsc`, which assumes `extern "C"` calls never throw.
- **Our own JPEG decoder leaves libjpeg by `setjmp`/`longjmp`.** The thumbnail decoder (`crates/deptheos_dng/native/jpeg_c_api.cpp`, `deptheos_dng::jpeg::decode_scaled`) sets an error manager that jumps back to its `setjmp`, from a frame holding nothing with a destructor, so a corrupt JPEG ends its decode with an error, not the process.
- **A per-user native archive cache** (`crates/native_build/src/native_cache.rs`, shared by both build scripts) keeps the compiled static libraries keyed by source hashes, build scripts, compiler version, target, profile and environment variables. A clean worktree's build drops from about 5.5 minutes (about 725 MB) to about 5 seconds. Our own bridge files compile on every build.
- XMP's expat is compiled with `BanAllEntityUsage`, against XML entity expansion attacks.

### The license

`LICENSE.md` grants three GPL-3.0 section 7 additional permissions:

1. **Adobe SDKs:** A covered work combined with the Adobe DNG SDK or XMP Toolkit (or modified versions of them) may be conveyed; the Corresponding Source of a non-source form includes the source of the SDK and toolkit parts used.
2. **LibRaw:** A covered work combined with LibRaw (or a modified version of it), used under CDDL-1.0, may be conveyed; the Corresponding Source of a non-source form includes the source of the LibRaw parts used. LibRaw is taken under CDDL-1.0 in every build, so every platform's notices are the same; CDDL-1.0 asks that LibRaw's files, our fork's changes included, stay available as source, which the source archive and the fork do.
3. **App stores:** A covered work may be conveyed in non-source form through Apple's App Store or another app distribution platform, even where the platform's terms of service or technical measures restrict recipients in ways the GPL would not otherwise allow, provided the Corresponding Source is available to anyone at no charge under the license and the work or its listing states where. This permission is granted to all recipients so forks may use it under their own names.

- **`CONTRIBUTING.md`:** Opening a pull request licenses the contribution under GPL-3.0 with all three section 7 permissions while the contributor retains copyright.
- **Trademarks outside the GPL (`TRADEMARKS.md`):** The "Deptheos" name and logo are not licensed under the GPL. Forks are welcome under their own names; distributing a modified build or an app-store listing under the Deptheos name requires permission.
- **Generated third-party notices (`THIRD_PARTY_NOTICES.txt`):** Generated by `script/notices.sh` from the vendored libraries' license files (including the IJG credit, MD5 notice, and `libjxl` patent grant) and Rust crate licenses via `cargo-about` (`licenses/about.toml`). `script/notices.sh --check` runs in CI and release workflows so a new dependency cannot ship without its notice. Every platform bundle includes `LICENSE.md` and `THIRD_PARTY_NOTICES.txt`.

## Alternatives considered

### System package managers or `vcpkg`

Their packages vary across platforms, complicate reproducible builds on macOS, iOS, Windows and Linux, and risk linking `libjpeg-turbo` into a DNG SDK written against IJG `libjpeg` 9c's internal structs.

### Building without XMP (`qDNGUseXMP=0`)

Deptheos writes an XMP packet into exported files recording the stacking method and frame count (ADR-133). Only `XMPFiles` is disabled.

### A C++ exception, not `longjmp`, out of our own JPEG decoder

A throw through libjpeg's C frames ends the process wherever those frames have no unwind tables, so a corrupt JPEG could crash the app; a jump back to a frame that owns nothing needs no unwinding at all, so our own decoder stays safe whatever a compiler assumes about `extern "C"` calls and whatever flags a build passes. The SDK's handler, which is Adobe's code, keeps its throw, and the build gives it the unwind tables it needs.

### LibRaw under LGPL-2.1

The license LibRaw is most often used under, and compatible with GPL-3.0 without a permission. Its relinking and no-further-restrictions terms don't hold for a statically linked App Store binary.

### A permissive license (MIT or Apache-2.0)

Would allow closed-source derivatives of the stacking engine and apps.

### A non-commercial or source-available license

Not an open-source license and incompatible with GPL dependencies.

## Consequences

- The first build on a machine compiles the C and C++ sources (about 5 minutes); later checkouts and worktrees hit the per-user native cache.
- Platform compiler defines live in `build.rs` and must be checked on macOS, Windows, Linux and iOS whenever the vendored SDK is updated.
- Redistributors conveying a binary linked with the DNG SDK must include the SDK and XMP source files used as part of the Corresponding Source.
- Adding a Rust crate or updating a vendored C/C++ library requires running `script/notices.sh` and passing `script/notices.sh --check` in CI. The notices list LibRaw for every build, the iOS app's included.

## Code

- `crates/deptheos_dng/build.rs`, `crates/deptheos_raw/build.rs`, `crates/native_build/src/flags.rs`, `crates/native_build/src/native_cache.rs`.
- `crates/deptheos_dng/native/jpeg_c_api.cpp`, `crates/deptheos_dng/src/jpeg.rs`.
- `native/vendor/PRUNED.md`, `.gitmodules`.
- `LICENSE.md`, `CONTRIBUTING.md`, `TRADEMARKS.md`, `THIRD_PARTY_NOTICES.txt`.
- `script/notices.sh`, `licenses/about.toml`, `.github/workflows/release.yml`, `.github/workflows/web.yml`.
