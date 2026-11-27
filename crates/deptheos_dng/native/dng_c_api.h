// Plain-C API over the DngBridge (dng_bridge.h) for the Rust side
// (src/lib.rs). The bridge's header is C++, which Rust cannot bind, so the
// decoded result stays behind an opaque handle: Rust never reads the
// metadata, only carries it from a source file to the written result.

#pragma once

#include <stdint.h>
#include <stddef.h>

// DtRawMetadata: deptheos_raw's C layout of LibRaw metadata.
#include "raw_c_api.h"
// DtStatus: why a call failed, the last argument of each that can (may be NULL).
#include "dt_status.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DtDngImage DtDngImage;

// A white point in CIE 1931 xy. Where a call takes one, NULL is the white
// as shot (deptheos_as_shot_white).
typedef struct DtDngWhite {
    double x;
    double y;
} DtDngWhite;

// Initializes the DNG and XMP SDKs. Idempotent; every SDK entry point calls it.
void dt_dng_initialize(void);

// Routes dng_log to the caller (NULL silences), from any thread. `level` is
// a DngLogLevel (dng_bridge.h): 0 trace, 1 debug, 2 info, 3 warn, 4 error.
typedef void (*DtDngLogCallback)(int32_t level, const char *message);
void dt_dng_set_log_callback(DtDngLogCallback callback);

// The test hooks, built where build.rs defines DEPTHEOS_TEST_HOOKS to 1
// (every build but a release one without the `test-support` feature).
#if DEPTHEOS_TEST_HOOKS
// Nonzero: this thread's reads take the SDK's path instead of the bridge's
// faster ones, for tests that check the two agree.
void dt_dng_set_reference_reads(int32_t on);
// How this thread's last read of linear pixels took them: `stage1` a
// stage1_read value (-1 before any), `deferred_black_level` 0 or 1.
void dt_dng_last_read_path(int32_t *stage1, int32_t *deferred_black_level);

// For tests: the SDK's lossless JPEG encoding of the samples into `out`
// (its size returned; 0 when `capacity` is too small), and the bridge's
// decoder checked against the SDK's (deptheos_dng_compare_lossless_jpeg's
// codes, or -3 if it throws).
size_t dt_dng_encode_lossless_jpeg(const uint16_t *samples, uint32_t rows, uint32_t cols,
                                   uint32_t channels, uint32_t bits, uint8_t *out,
                                   size_t capacity);
int32_t dt_dng_compare_lossless_jpeg(const uint8_t *data, size_t size, size_t samples);

// For tests: `w` x `h` RGB pixels at (`x`, `y`) of the file's default crop,
// rendered to sRGB by the SDK from the file's own negative
// (deptheos_dng_render_file_srgb), into `out` (`w * h * 3` samples).
// `dcp_path`, if not NULL, replaces the file's profiles. 1 on success.
int32_t dt_dng_render_file_srgb(const char *path, const char *dcp_path, uint32_t x, uint32_t y,
                                uint32_t w, uint32_t h, uint16_t *out, size_t capacity);
// The file's DefaultUserCrop (top, left, bottom, right, of its default
// crop) and Orientation tag, as the SDK parses them
// (deptheos_dng_frame_of). 1 on success.
int32_t dt_dng_frame_of(const char *path, double *user_crop, uint32_t *orientation);
#endif // DEPTHEOS_TEST_HOOKS

// Supplies the buffer dt_dng_read_into decodes into: called once, on the
// reading thread, with the sample count (width * height * channels), it
// returns room for that many uint16_t, or NULL to fail the read.
typedef uint16_t *(*DtDngPixelAlloc)(void *ctx, size_t count);

// Decodes a DNG to linear 16-bit camera RGB straight into the buffer `alloc`
// returns, keeping its metadata. The handle holds no pixels
// (dt_dng_pixel_count == 0) but reports the buffer's width, height and
// channels; the buffer is filled when the handle is non-NULL with
// channels > 0. A CFA DNG comes back with channels == 0 (LibRaw demosaics
// it) and `alloc` uncalled. `bypass_cache` reads an uncompressed image
// around the OS file cache (Windows only). At most `threads` run (0: every
// core); a nonzero `cancel` (may be NULL) stops the read. An image or
// default crop over `max_pixels` is refused before allocating, with
// DT_ERROR_TOO_LARGE. Returns NULL on failure.
DtDngImage *dt_dng_read_into(const char *path, int32_t bypass_cache, uint64_t max_pixels,
                             DtDngPixelAlloc alloc, void *ctx, uint32_t threads,
                             const uint8_t *cancel, DtStatus *status);

// Parses the file's color science and geometry without its image: a
// `meta_source` with no pixels (dt_dng_channels == 0). NULL on failure.
DtDngImage *dt_dng_read_metadata(const char *path, DtStatus *status);

int32_t dt_dng_width(const DtDngImage *img);
int32_t dt_dng_height(const DtDngImage *img);
int32_t dt_dng_channels(const DtDngImage *img);
int64_t dt_dng_pixel_count(const DtDngImage *img);

// Borrowed pointer to interleaved 16-bit pixels (a rendered image's; a read
// keeps none), valid until dt_dng_free.
const uint16_t *dt_dng_pixels(const DtDngImage *img);

// Renders linear pixels (a composite) to sRGB with `meta_source`'s color
// science at `white` and `exposure` stops over the baseline
// (dng_render's SetWhiteXY and SetExposure), as a new image.
DtDngImage *dt_dng_render_linear_to_srgb(
    const DtDngImage *meta_source,
    const uint16_t *pixels,
    size_t pixel_count,
    int32_t width,
    int32_t height,
    int32_t channels,
    const DtDngWhite *white,
    double exposure,
    DtStatus *status);

// Builds DNG metadata from LibRaw's, so a stack of non-DNG RAW frames can be
// written to DNG. Matrices keep LibRaw's layouts (raw_c_api.h), a zero
// has-flag means the tag is absent, and with no DNG color matrices cam_xyz
// is the color science. Returns a metadata-only `meta_source` (no pixels,
// zero dimensions), or NULL on failure.
DtDngImage *dt_dng_from_raw_metadata(const DtRawMetadata *meta, DtStatus *status);

// Applies a DCP profile to `img`'s metadata in place: its matrices,
// illuminants, LUTs, name and baseline-exposure offset replace the file's;
// camera calibration and as-shot neutral stay. Returns 1 on success, 0 when
// the profile can't be parsed.
int32_t dt_dng_apply_dcp(DtDngImage *img, const char *dcp_path, DtStatus *status);

// How a DtDngField's value is held.
enum {
    DT_DNG_FIELD_TEXT = 1,       // `text`, NUL-terminated
    DT_DNG_FIELD_INTEGERS = 2,   // `count` values in `numbers` (BYTE, SHORT or LONG)
    DT_DNG_FIELD_RATIONALS = 3,  // `count` numerator/denominator pairs in `numbers`
    DT_DNG_FIELD_SRATIONALS = 4, // the same, each part an int32_t's bits
};

// One tag an export copies from its reference frame, as the frame holds it
// (the JPEG and TIFF exports' allowlist): IFD0's or the Exif IFD's, by tag
// number. A tag the writer has no place for is skipped.
typedef struct DtDngField {
    uint16_t tag;
    uint16_t kind;
    uint32_t count;
    const char *text;
    const uint32_t *numbers;
} DtDngField;

// What dt_dng_write adds for an exported result: `software` replaces the
// Software tag, `user_comment` and `xmp` are added, `capture_time` (ISO
// 8601) fills a missing DateTimeOriginal, and `fields` replace the carried
// metadata's same tags, so the export names camera, lens and exposure as
// the frame does (LibRaw normalizes the model name). `white`, if not NULL,
// is written as AsShotNeutral: its camera neutral under the carried
// profile (none for a monochrome file or one without a profile). Each
// string may be NULL or empty. Layout matches the Rust declaration
// (src/lib.rs).
typedef struct DtDngTags {
    const char *software;
    const char *user_comment;
    const char *xmp;
    const char *capture_time;
    const DtDngField *fields;
    size_t field_count;
    const DtDngWhite *white;
    // The DefaultUserCrop as left, top, width and height of the pixels
    // written, or NULL for the whole.
    const uint32_t *user_crop;
    // The Orientation tag, 1 to 8.
    uint32_t orientation;
} DtDngTags;

// Writes pixels to a DNG with `meta_source`'s metadata, minus GPS and the
// body's and lens's serial numbers. With `tags` (an export) they are added
// and opcode list 3 kept only at the size it applies to; with NULL (the
// Library's color reference) nothing is added and opcode list 3 is kept
// (DngWriteKind, dng_bridge.h). A failed write can leave `out_path` partly
// written. Returns 1 on success.
int32_t dt_dng_write(
    const DtDngImage *meta_source,
    const char *out_path,
    const uint16_t *pixels,
    size_t pixel_count,
    int32_t width,
    int32_t height,
    int32_t channels,
    const DtDngTags *tags,
    DtStatus *status);

void dt_dng_free(DtDngImage *img);

// The SDK's dng_color_spec output for the GPU render. Layout matches
// src/lib.rs. The arrays are owned by the C++ side; free the whole struct
// with dt_dng_render_params_free.
typedef struct DtDngRenderParams {
    float camera_to_prophoto[9]; // row-major camera RGB -> ProPhoto RGB
    float camera_white[3];
    double black_level;
    double baseline_exposure; // stops
    double shadow_scale; // ShadowScale x Stage3Gain; 0 when the profile's
                         // DefaultBlackRender turns the shadow ramp off

    // HueSatMap for the shot's white point: (hue shift degrees, sat scale,
    // val scale) triples, hue_sat_count floats. Null when absent.
    uint32_t hue_sat_dims[3]; // hue, sat, val divisions
    uint32_t hue_sat_encoding; // 0 linear, 1 sRGB (ProfileHueSatMapEncoding)
    float *hue_sat_data;
    int64_t hue_sat_count;

    uint32_t look_table_dims[3];
    uint32_t look_table_encoding; // 0 linear, 1 sRGB (ProfileLookTableEncoding)
    float *look_table_data;
    int64_t look_table_count;

    // Profile tone curve (or the default ACR curve) sampled at 257 points.
    float tone_curve[257];
} DtDngRenderParams;

// The parameters at `white` (NULL: as shot). NULL on failure.
DtDngRenderParams *dt_dng_compute_render_params(const DtDngImage *meta_source,
                                                const DtDngWhite *white,
                                                DtStatus *status);

void dt_dng_render_params_free(DtDngRenderParams *params);

// The white the render takes as shot, in `out`, with `has` 1; `has` 0 for a
// monochrome file or one without a usable profile. Returns 1 on success.
int32_t dt_dng_as_shot_white(const DtDngImage *meta_source, DtDngWhite *out, int32_t *has,
                             DtStatus *status);

// The SDK's temperature (kelvin) and tint of `white` (dng_temperature); a
// white not finite, or with x or y not positive, reads as D50. Returns 1 on
// success.
int32_t dt_dng_temperature_of(DtDngWhite white, double *kelvin, double *tint);

// The white of a temperature and tint (dng_temperature); a kelvin not
// finite and positive, or a tint not finite, reads as D50. Returns 1 on
// success.
int32_t dt_dng_temperature_white(double kelvin, double tint, DtDngWhite *out);

#ifdef __cplusplus
} // extern "C"
#endif
