# Pruned vendored files

The libraries here are upstream releases with the files the build never
compiles or includes removed. The DNG SDK, XMP toolkit, libjpeg and libjxl
all come from Adobe's DNG SDK 1.7.1 package, build 2768
(`dng_sdk_1_7_1_2768_20261013.zip`); its `LICENSE.txt` and `DNG_ReadMe.txt`
sit in `dng_sdk/`, and each library keeps the package's `*_ReadMe.txt` for it.
That `LICENSE.txt` covers the XMP Toolkit too. The Boost 1.60 headers in
`xmp/toolkit/XMPCore/third-party/boost` are under the Boost Software License
1.0 (`licenses/boost-LICENSE_1_0.txt`).
The sources are unmodified apart from the pruning below and the newer expat
(see "Replaced"). The pruning removed whole files only; it
changed nothing that is compiled. What is compiled is listed in
`crates/deptheos_dng/build.rs`.

Removed on 2026-11-03:

- **libjpeg** (IJG 9c): everything except the library's `.c` and `.h` files
  and `README`: the autotools and makefiles for other platforms, the
  documentation (`*.txt`, man pages), the test images and the DOS assembly.
  The command-line tools' sources are still present but not compiled.
- **libjxl**: `lib/jpegli`; `client_projects/mac` and `client_projects/win`
  (Adobe's Xcode and Visual Studio projects; `client_projects/include` is
  used); `prune.txt`. In `third_party/brotli`: the JavaScript, C#, Go and
  Python bindings, `scripts`, `setup.py` and `configure-cmake`.
- **DNG SDK**: `projects` (Xcode and Visual Studio) and `documents`.
- **XMP Toolkit**: XMPFiles (the DNG SDK is built with `qDNGXMPFiles=0`),
  XMPFilesPlugins, XMPScript, XMPCompareAndMerge, XMPExtensions and
  XMPWasm.

Removed on 2026-11-11:

- **XMP Toolkit**: the four expat release tarballs in
  `XMPCore/third-party/expat/public` (2.1.0, 2.2.0, 2.2.4 and 2.2.9).
  The build compiles the unpacked sources in `public/lib`, which were
  2.2.9 then and are 2.10.0 now (see "Replaced").

Replaced on 2026-11-12:

- **XMP Toolkit**: expat 2.2.9 in `XMPCore/third-party/expat/public/lib`
  (from 2019, with the package's changes) with the `lib` sources and headers
  of the expat 2.10.0 release, unchanged (`expat-2.10.0.tar.gz`, SHA-256
  `5466a341f9c692ef7a958a6881311be597f8e1ce345984cf78414f54793dea4c`), for
  the security fixes since. The package's changes to 2.2.9 are not carried
  over: they picked each system's entropy source, which `build.rs` now does
  with defines, and capped nested entity references, which expat does itself
  since 2.4 and the toolkit's adapter makes moot by refusing any DOCTYPE.
  To update, copy a release's `lib/*.c` and `lib/*.h` over these.
