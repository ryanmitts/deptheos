# ADR-132: Every decoder of standard images hands over the same thing, and one input stage turns it into a frame in the stack's color space

## Status

Accepted.

## Context

A standard image (JPEG, PNG, TIFF; HEIC on iOS) reaches the stackers through the desktop's and CLI's decoders and through iOS's ImageIO decoder. If each decides color, alpha, depth and orientation on its own, the apps disagree on what a frame is, a stack is sized from a header that disagrees with the pixels it then gets, wide gamut is clipped on one app and kept on the other, and linear inputs are stacked as if they were gamma encoded, which is what ADR-130 encodes RAW to the sRGB curve to avoid. A pre-aligned frame's transparent border must not be stacked as image.

The PNG specification ranks `cICP` above `iCCP`, `sRGB`, `cHRM` and `gAMA` (W3C PNG Third Edition, §4.3 and §11.3.2.6).

## Decision

### One handover from every decoder

Every decoder, on both apps and for every standard format, produces one `Decoded` (`crates/deptheos_core/src/io/input.rs`): a header (the size as stored, before orientation; the stored depth, 8, 16 or 32-bit float; the layout, gray or RGB with or without alpha; the color tag, an ICC profile, a `cICP`, a PNG's `sRGB`, `gAMA` and `cHRM` chunks, the DCF Adobe RGB hint, or none; and the orientation, EXIF's or a HEIF container's) and the samples, interleaved, as stored.

Alpha, and whether it is premultiplied, is part of the layout, not a flag beside it. A decoder opens a file once (`OpenFrame` in `crates/deptheos_core/src/io/decoders.rs`): the header and then the pixels come from the same open, so the buffer is sized from the state that fills it. A format that can't be read this way is refused by name.

### One input stage turns it into a frame

Two functions turn it into a frame, each told whether the frame is its own (`Target::Own`) or joins a stack's space (`Target::Stack`).

`input_header` gives the frame's upright size and the depth it will come out at, and refuses what can't be stacked; it runs when a stack is planned, from headers alone, and the session sizes the stack from it. `input_frame` makes exactly the frame its header promised, so header and frame can't disagree; a frame that comes out otherwise is an error naming the file, never an assert. A `Frame` holds 8- or 16-bit RGB, its `IccProfile` (`None` for sRGB), its data area and its `LoadNote`s. `StackSpace::of` takes the first frame whose header reads. `Target::Own` serves one-off loads: previews, thumbnails, the CLI's single files.

In order, `input_frame`:

1. **Orientation, once.** It is applied to the pixels here and nowhere else; no decoder of standard images turns pixels. For HEIF it is the container's (`irot`/`imir`), never EXIF's, which a writer may leave stale.
2. **Alpha.** Alpha is *binary* when every pixel is fully transparent or fully opaque, but for a band at most 2 px wide next to transparent pixels (`EDGE_BAND`, a warp's antialiased edge). A binary frame's data area is the largest axis-aligned rectangle of opaque pixels. It joins the aligner's valid area (`PreparedFrame::with_data_area`), the area every frame covers, and goes through the crop rule (a quarter or more of the frame is cropped to; less leaves the render uncropped and says so). The pixels outside it are filled from the nearest opaque edge, so neither the aligner nor the pyramid sees a hard step there. Any other alpha is *partial* and is composited over white in the frame's encoded values: `a·c + (1 − a)` for straight alpha, `c + (1 − a)` for premultiplied, which then needs no division. A partial frame is noted (`LoadNote::OverWhite`).
3. **Linear transfer.** A frame whose tone curves are all the identity (a linear profile, `cICP` transfer 8, `gAMA` 1.0, or float without a tag, taken as linear sRGB) is encoded to the sRGB curve, keeping its primaries, and gets a profile of those primaries and sRGB's curve, at 16 bits. The curve is IEC 61966-2-1's, evaluated in `f32` without clamping and mirrored below zero (the extended sRGB form), so step 5's conversion sees values outside 0–1 as they are. A 16-bit store can't keep a value above 1: at quantization it clips to 1 (and below 0 to 0), once, with `LoadNote::FloatClipped`. The sRGB curve's fine steps near black keep an 8-bit linear frame's shadow levels apart at 16 bits.
4. **Gray.** A gray frame's space is an RGB profile made from its gray curve (sRGB primaries, D65, the gray profile's tone curve; sRGB's curve when untagged), and its values are expanded to R = G = B unchanged. An all-gray stack keeps its values, and its exports embed that RGB profile. A gray frame in an RGB stack is converted by step 5 like any frame in another space.
5. **Another space.** A frame whose space is the stack's (`IccProfile::same_space`, sRGB included) is not transformed, so its values stay exactly as stored, at their depth. Any other frame is converted into the stack's space at 16 bits, relative colorimetric, and noted when its colors fall outside it (`LoadNote::Clipped { from, into }`). The stack takes the first frame's space, never the widest: putting a wide frame first is how the user keeps the gamut.

**Refused, with a message naming the file and what it is** (`Refused`), from the header, before anything is decoded, and the stack is not started: CMYK (JPEG CMYK and YCCK, TIFF CMYK), Lab, a TIFF layout that isn't read (palette, subsampled YCbCr outside JPEG compression, signed samples, more bands than gray or RGB with one alpha), and HDR transfer (PQ or HLG, in a HEIF's color, a PNG's `cICP` or an ICC profile's `cicp` tag). CMYK is refused rather than converted: no camera writes it, a conversion only guesses at the press it was separated for when the file names none, and a stack in a printer's gamut is narrower than its own source was.

### The color library

`moxcms` does every conversion. Its `f32` matrix-shaper transforms with `allow_extended_range_rgb_xyz` (its `extended_range` feature) evaluate parametric curves without clamping, for steps 3 and 5. LUT-based profiles clamp in it, and the clip is noted the same way. Its gray-to-RGB transforms serve step 4, and `cicp_profile` (`crates/deptheos_core/src/io/profile.rs`) builds a profile from a PNG's `cICP`. The 16-bit fixed-point executor serves the display's `ToSrgb`.

### Decoders

- **Desktop and CLI** (`crates/deptheos_core/src/io/decoders.rs`). A JPEG goes through `image`'s JPEG decoder, with a segment walk for its component count, ICC and EXIF and a truncated file. A PNG goes through the `png` crate, its color chunks ranked as the specification orders them. A TIFF goes through the `tiff` crate, its ICC, orientation and kind of alpha read by the decoder that reads the pixels. It also reads planar RGB, 32-bit float, 32-bit integer gray (microscopy writes all three), unsubsampled YCbCr through its coefficients, and JPEG-compressed YCbCr. One decode allocates at most 4 GiB (`MAX_DECODE_ALLOC`). HEIC and WebP are not read on the desktop.
- **iOS** (`AppleFrameDecoder.swift`, `BitmapDecode.swift`). A `CGImageSource` is opened once per frame; an `OpenedFrame` across the bridge (`crates/deptheos_ffi/src/decode.rs`) gives the header and then draws the pixels from that same source. It draws as stored, without turning, into the image's own color space, or Display P3 when a bitmap context can't be made in that space, and hands over the space's ICC bytes (`copyICCData`). Gray draws into a gray context; float TIFFs into a 32-bit float context. Alpha draws as `premultipliedLast` (the only RGB alpha a bitmap context takes) and is said to be premultiplied. An iPhone HEIC is read with `kCGImageSourceDecodeToSDR`, its standard-range image without the gain map applied. A space with an ITU-R BT.2100 transfer (`CGColorSpaceUsesITUR_2100TF`) is refused as HDR, and CMYK and Lab before drawing. The orientation handed over is the source's `kCGImagePropertyOrientation`, which for HEIF must be the container's.
- **RAW is not this stage's.** Apple's RAW decoder renders camera RAW upright into Display P3 at 16 bits and hands over P3's profile. LibRaw and the DNG SDK keep their own path: linear camera RGB in the file's color science, upright from their decoders (ADR-130, ADR-131).

### Metadata: two readers

- **`kamadak-exif`** reads a frame's EXIF block, handed over with the pixels, for orientation, the DCF `R03` Adobe RGB hint and a JPEG's EXIF thumbnail. That block is a JPEG's APP1, a PNG's `eXIf` or a HEIF's `Exif` item. A TIFF's tags come from the `tiff` decoder's own IFDs: `kamadak-exif` reads a whole TIFF into memory to parse it, too much for a 300 MB frame on a share.
- **`capture.rs`** (`crates/deptheos_app/src/frames/capture.rs`) reads capture time, model and burst shot for auto sort, from small blocks off slow cards (CR3's `CMT` boxes, RAF's preview, maker-note drive modes), and the tags an export copies, for every format: a PNG's `eXIf` reaches exports by its finding the chunk (`png_exif` in `tiff_streams`).

### Alongside

Desktop source thumbnails go through `input_frame` with `Target::Own`, then `ToSrgb`, so they are color managed and take their orientation from this stage; an EXIF thumbnail is converted with its main image's color tag.

## Alternatives considered

### Refuse a frame in another color space

Simple and lossless. But a stack shot across two sessions, or from an editor that tags some exports differently, stopped with an error the user could act on only outside the app. Converting into the first frame's space at 16 bits keeps every color that space can hold and says when one couldn't be kept.

### Convert every frame to one fixed wide space (ProPhoto RGB or linear Rec. 2020)

No clipping ever. But an sRGB stack, the common case, would be transformed and re-rounded, could no longer stay bit-identical or 8-bit, and its exports would carry a space the user never chose.

### Stack linear inputs in a float source, keeping values above 1

The GPU could hold them (`Rgba32Float` sources exist for adapters without `Rgba16Unorm`). But every stacker, the 16-bit exports and the display assume 0–1, and a value above white has no meaning in the display-referred spaces exports are tagged with. Clipping once, with a note, is what a 16-bit stack can honestly do.

### Convert CMYK through a profile

It worked only with a profile, needed a system profile otherwise, and the 16-bit RGB it gave disagreed with the 8-bit header the stack was sized from. For input no camera produces, refusing by name is the clear answer.

### Composite all alpha over white

Simpler, but a pre-aligned frame's empty border would become a white band stacked as image. Its alpha says where it has no data, and the common area already handles exactly that.

### Each app keeps its own decoder rules

Alpha, gray and gamut then differ between the apps.

### Read the tags an export copies with `kamadak-exif` too

It would add a second path for standard frames' export tags without removing one, since `capture.rs` must still read a JPEG's and a HEIF's EXIF for auto sort.

## Consequences

- The stacker golden tests (ADR-113) build their frames in memory and upload them directly, so this stage changes none of their inputs. Opaque 8-bit sRGB frames (JPEG, PNG, TIFF) skip every step but orientation, so their pixels, and the alignment computed from them (ADR-112), are bit-identical to a plain decode (`opaque_srgb_frames_load_bit_identically`).
- Linear, gray-with-a-profile, alpha, premultiplied, wide-gamut and other-space frames each have a pinned test of `input_frame`'s output on a generated fixture. `rgba16_source_aligns_like_its_rgb_twin` uses opaque alpha, so it still tests that all 16 bits reach the aligner.
- Applying a binary alpha's border to the stackers' footprints too, so an uncropped render takes nothing from the filled border, would change stacker output. It belongs to stacking quality and its blind vote (ADR-120).
- A frame converted from 8 bits stacks at 16, so a JPEG's depth comes from its header (its ICC and EXIF, in the first segments), not from its name.
- On iOS an sRGB frame draws into sRGB and stacks as on the desktop; wide-gamut frames keep their gamut, and their exports embed the profile.
- A frame in another color space is converted, not a reason to stop the stack; a frame of another size still stops it (ADR-121).

## Code

- `crates/deptheos_core/src/io/input.rs`, `crates/deptheos_core/src/io/decoders.rs`, `crates/deptheos_core/src/io/load.rs`, `crates/deptheos_core/src/io/profile.rs`.
- `crates/deptheos_aligner/src/lib.rs`, `crates/deptheos_app/src/runs/session.rs`.
- `crates/deptheos_app/src/frames/source_thumbnail.rs`, `crates/deptheos_app/src/frames/capture.rs`.
- `crates/deptheos_ffi/src/decode.rs`, `ios/DeptheosMobile/Engine/AppleFrameDecoder.swift`, `ios/DeptheosMobile/Engine/BitmapDecode.swift`.
- Tests: `crates/deptheos_core/src/io/input.rs`; `crates/deptheos_core/tests/integration/decode.rs`, `alignment.rs`.
