# ADR-133: Exports are encoded by Rust crates with metadata we write ourselves: the reference frame's allowlisted tags, nothing that identifies the owner, source files by name, and an ordered dither at 8 bits

## Status

Accepted.

## Context

A finished stack exports as JPEG, PNG, TIFF or DNG. A DNG is written by the Adobe DNG SDK from the reference frame's metadata (ADR-130, ADR-131). For JPEG, PNG and TIFF, the standard Rust encoders (`image`, `png`, `tiff`) write no EXIF or XMP, `image`'s JPEG encoder is single-threaded (about 0.5 s for a 36 MP image), and `tiff` writes uncompressed by default.

An export is copied, shared and uploaded, and so is the Library it came from: what its metadata says about the camera's owner, the place and the folders on the user's disk goes with it.

A stack is 16 bits a channel until it is saved. Rounding to 8 bits turns a smooth area (an out-of-focus background, a gray card, a sky), where a gradient spans a few codes across hundreds of pixels, into flat bands with visible steps that the 16-bit result does not have, and editors that read the export stretch those steps further. The same rounding (`u16_to_u8`) feeds the apps' display, which is redrawn and compared pixel for pixel and must stay as it is.

## Decision

### Encoders (`crates/deptheos_core/src/io/`)

- **JPEG** (`jpeg.rs`, `encode_jpeg`): `image`'s JPEG encoder runs on parallel horizontal bands of whole 8×8 MCU rows, joined into one scan with restart markers between bands, giving output bit-identical to a single-threaded encode.
- **PNG** through the `png` crate.
- **TIFF** (`tiff.rs`) through the `tiff` crate, with the horizontal differencing predictor and parallel strips of about 1 MiB (`TIFF_STRIP_BYTES`), compressed with ZIP/Deflate (the default), LZW or nothing. An output past classic TIFF's 4 GB is refused.
- Every export is written atomically through `write_replacing` (ADR-141).

### Metadata written (`io/metadata.rs`, `crates/deptheos_app/src/library/export_tags.rs`)

- **Embedded:** EXIF (a JPEG's APP1, a PNG's `eXIf` chunk, a TIFF's own IFD tags), an XMP packet (JPEG APP1, PNG `iTXt`, TIFF tag 700) and the ICC profile (JPEG APP2, PNG `iCCP`, TIFF tag 34675; ADR-132).
- **Copied from the reference frame, by allowlist** (`COPIED_IFD0`, `COPIED_EXIF` in `crates/deptheos_app/src/frames/capture.rs`): the camera's make and model, `Artist` and `Copyright`, and the exposure, capture-time, focal-length and lens tags. They come from the first stacked frame, skipping a left-out leading frame: its JPEG or HEIF Exif block, a PNG's `eXIf` chunk, a TIFF's own IFDs, or a RAW or DNG file's metadata.
- **A DNG export writes the same copied tags** (`DngTags::fields`, from `export_tags::dng_fields`) over what its source metadata says for them, so it names the camera, lens and exposure as the frame does. `UniqueCameraModel` stays LibRaw's normalized name, which the color profiles are matched by, as the DNG converter's own files have it beside the frame's `Model`. A lens's unknown aperture is written 0/1, never 0/0, and a focal length LibRaw has as 0 is unknown, so a lens without one gets no `LensSpecification` of zeros. A capture time from the tags fills `DateTimeOriginal` when the source metadata has none (LibRaw's carries none). A render's white balance, when set, is written as its as-shot white (`AsShotNeutral`, `DngTags::white`). Its reframe keeps every pixel: the crop is written as the `DefaultUserCrop` and the flip and turn as the `Orientation` tag (`DngTags::user_crop`, `DngTags::orientation`, from `Reframe::orientation`), which a raw editor shows and can undo (ADR-136). No other adjustment reaches a DNG, whose pixels stay as stored (ADR-137).
- **Never written, in any format:** per-frame focus distance, maker notes, GPS coordinates, `CameraOwnerName`, `BodySerialNumber` and `LensSerialNumber`. Standard exports carry only the allowlist. Neither the DNG bridge nor the LibRaw one reads GPS or the body's and lens's serial numbers (`DngCameraInfo` and `DngLensInfo` in `crates/deptheos_dng/native/dng_bridge.h`, `DtRawMetadata` in `crates/deptheos_raw/native/raw_c_api.h` have no place for them), so no DNG written holds them, the Library's 16×16 `color.dng` (ADR-130) included.
- **Added for the stack:** `Software` and `xmp:CreatorTool` (`"Deptheos <version>"`), the modification date, `Orientation = 1`, the color space tags, an EXIF `UserComment` summing the stack up, and XMP `xmpMM:Ingredients` listing the source files by basename, never a directory path, plus `deptheos:Method`, `deptheos:FrameCount` and `deptheos:ReferenceFrame` in `https://deptheos.com/ns/stack/1.0/`. The packet is held to 60,000 bytes so it fits one JPEG APP1 segment; ingredients past that are left out and `deptheos:IngredientsTruncated="True"` says so.

### A render exports under its saved adjustments

- **An export of a Library render draws it under its adjustments file** (`Look::saved`, ADR-152): its sliders and its reframe, read on the store thread after every write sent before (`History::adjustments_to_export`), so an export made while Adjust is open has the last change. Its pixels are those the viewer's settled full picture draws, in the export's space, before dither. A damaged or unreadable file fails the export with a worded reason (`Error::AdjustmentsUnread`) rather than export without the adjustments. A result not yet in the Library, a batch's own exports of what it just stacked and aligned frames export with none.
- **An adjusted camera RAW render is written in the space chosen** (`ExportSpace`, `Look::saved_in`): sRGB, Display P3, Adobe RGB or ProPhoto RGB, each with its profile embedded; ProPhoto only at 16 bits, where its gamut has the codes, so at 8 it writes Adobe RGB. Unadjusted, it is sRGB as before. 16 bits are offered whenever an adjustment is set, as the adjusted draw has them even from 8-bit frames. An adjusted standard stack keeps its own profile, but for one drawn through tables (`adjusted_in_srgb`), which has no curves to encode back through and is written in sRGB, as the sheet says. A DNG of an adjusted render carries only its white balance and crop, the tags a DNG has, and the sheet says so. Whether the sheet offers a space and which (`ExportSpace::offered`, `ExportSpace::for_format`) is shared, and the choice is kept with the export preferences as `export_space` (ADR-150). Each app's sheet reads whether the render is adjusted off the UI thread as it opens (`crates/deptheos_gpui/src/export_dialog.rs`, `ios/DeptheosMobile/Views/ExportSheet.swift`).

### 8-bit saves are dithered (`crates/deptheos_core/src/io/save.rs`)

- **An 8-bit save adds an ordered dither before it quantizes** (`dither_rgb_to_u8`, from `save_rgb16`): each value `v · 255 / 65535` plus an offset in (−½, ½) LSB, `(2·rank + 1)/128 − ½`, from the pixel's place in the standard 8×8 Bayer matrix (`BAYER8`), then floored. Integer arithmetic throughout.
- **The offset is a function of position alone.** The same pixels give the same bytes on every machine and run, so two exports of one render are the same file, and a pixel's three channels share the offset, so a neutral gray stays neutral.
- **A value already on an 8-bit code comes out as that code**, black and white included: an 8-bit source re-exported at 8 bits is unchanged.
- **Only the 8-bit export path**: a render's JPEG, 8-bit PNG and TIFF, and a turntable or grow sequence's frame files at 8 bits, all through `save_rgb16`. 16-bit PNG and TIFF and DNG are not quantized; a movie's frames go to the encoder at 16 bits; the apps' display and thumbnails keep plain rounding.

## Alternatives considered

### Linking `libjpeg-turbo` or `mozjpeg` for JPEG exports

Parallel MCU-row encoding with restart markers in Rust (`jpeg.rs`) makes JPEG export fast without adding another C library to the image I/O layer.

### Including directory paths or serial numbers in exported metadata

Leaving out directory paths, `CameraOwnerName`, `BodySerialNumber`, `LensSerialNumber` and GPS keeps local usernames, hardware serial numbers and locations out of shared exports and Libraries, while `Artist` and `Copyright`, which the photographer set to be seen, stay (`source_tags_leave_out_the_owner_and_serial_numbers`, `a_write_leaves_out_gps_and_serial_numbers`).

### Blue-noise dither

A blue-noise tile has no visible period, where Bayer's 8-pixel grid can show in a flat area pushed hard in an editor. It needs a precomputed tile in the source (64×64 is 4 KB) and tests that hold its spectrum. At ±½ LSB the Bayer pattern is below what a screen or print shows, so the simpler table was taken; swapping the table is a local change.

### Error diffusion (Floyd–Steinberg)

Lower error at each pixel, but every pixel depends on the ones before it, so it runs serially where the export quantizes rows in parallel, and a change anywhere changes every pixel after it, so two nearly equal renders export to very different bytes.

### Random noise

Breaks up bands as well, at more visible grain for the same amplitude, and needs a fixed seed to stay deterministic. It does nothing ordered dither doesn't.

### Leaving 8-bit exports rounded

Banding in 8-bit exports is what users see, and JPEG has no 16 bits.

## Consequences

- Photo managers read the camera, lens, exposure, capture date, ICC profile and stacking ingredients on every exported JPEG, PNG, TIFF and DNG. The end-to-end checker holds every kept tag to a frame's value and every private one gone (`crates/deptheos_e2e`).
- A tag not on the allowlist is not written, however harmless; adding one is a change to `COPIED_IFD0` or `COPIED_EXIF`, and the DNG writer skips a tag it has no place for.
- A slow gradient exports as an even, fine pattern between neighboring codes that averages to the 16-bit value (`dither_breaks_up_bands_on_a_slow_ramp`: a ramp of four codes over 4096 pixels is five bands over a hundred 8×8 blocks wide when rounded, and steps a 64th of a code at a time when dithered, each block within a tenth of a code of the 16-bit mean).
- Exports stay byte-for-byte reproducible (`dither_is_deterministic_and_keeps_exact_codes`).
- A JPEG of a smooth area holds a little more high-frequency detail, so files may grow slightly; not measured.
- An 8-bit export is not the display's pixels exactly: tests that compare them use `dither_rgb_to_u8`.

## Code

- `crates/deptheos_core/src/io/jpeg.rs`, `tiff.rs`, `save.rs`, `metadata.rs`.
- `crates/deptheos_app/src/library/export_tags.rs`, `crates/deptheos_app/src/frames/capture.rs`.
- `crates/deptheos_dng/src/lib.rs`, `crates/deptheos_dng/native/dng_c_api.cpp`, `crates/deptheos_dng/native/dng_bridge.h`, `crates/deptheos_dng/native/dng_write.cpp`.
