// Plain-C API over LibRaw for src/lib.rs. Safe on any thread, small stacks
// included: LibRaw's state is on the heap. A decode copies the pixels once,
// into the caller's memory: dt_raw_decoder, dt_raw_decode,
// dt_raw_copy_pixels, dt_raw_decode_free. The handle comes first so another
// thread can stop the decode (dt_raw_interrupt).

#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Paths are UTF-16 on Windows (LibRaw's wide-path open_file), raw bytes
// elsewhere. NUL-terminated either way.
#ifdef _WIN32
typedef wchar_t DtRawPathChar;
#else
typedef char DtRawPathChar;
#endif

// Camera and color metadata. Strings are NUL-terminated and truncated to fit.
// No serial numbers or location are read. int32_t flags, not bool, for a
// stable ABI across compilers.
typedef struct DtRawMetadata {
    char make[64];
    char model[64];
    char software[64];
    // The name the camera goes by in a DNG's UniqueCameraModel and in
    // Adobe's profile names (dt_raw_camera_name): the file's own
    // UniqueCameraModel, else "<make> <model>".
    char unique_camera_model[64];
    char artist[64];

    char lens_make[128];
    char lens_model[128];
    float lens_min_focal;
    float lens_max_focal;
    int32_t focal_length_in_35mm;

    float iso_speed;
    float shutter;   // seconds
    float aperture;  // f-number
    float focal_len; // mm
    int32_t raw_colors; // 3 or 4

    float cam_mul[4];  // camera white balance R,G1,B,G2
    float cam_xyz[12]; // 4x3 row-major XYZ->camera (D65, Adobe color matrix)
    float as_shot_neutral[4];
    int32_t as_shot_neutral_count;

    // Dual-illuminant DNG color science (LibRaw dng_color[0..1]). A zero
    // has-flag means the file carries no such tag and the matrix is all
    // zeros. Layouts are LibRaw's own: color matrix 4x3 row-major
    // (XYZ->camera), forward matrix 3x4 row-major (camera->XYZ), calibration
    // 4x4 row-major.
    uint32_t dng_illuminant[2];
    float dng_color_matrix[2][12];
    float dng_forward_matrix[2][12];
    float dng_calibration[2][16];
    int32_t dng_has_color_matrix[2];
    int32_t dng_has_forward_matrix[2];
    int32_t dng_has_calibration[2];

    float analog_balance[4];
    int32_t analog_balance_count;
    float baseline_exposure; // valid only when has_baseline_exposure
    int32_t has_baseline_exposure;
    float linear_response_limit; // valid only when has_linear_response_limit
    int32_t has_linear_response_limit;
} DtRawMetadata;

// A decoded frame held by LibRaw until its pixels are copied out.
typedef struct DtRawDecode DtRawDecode;

// Functions returning int32_t return a LibRaw error code: 0 (LIBRAW_SUCCESS)
// on success, otherwise a code dt_raw_strerror describes.

// Metadata only: open_file() (which runs identify()), no unpack or demosaic.
int32_t dt_raw_read_metadata(const DtRawPathChar *path, DtRawMetadata *out);

// The upright size dt_raw_decode will return for `path`, from open_file()
// alone: identify() with the decode's params, no unpack or demosaic.
int32_t dt_raw_read_size(const DtRawPathChar *path, int32_t *width, int32_t *height);

// A decode's handle, NULL if there is no memory for one.
DtRawDecode *dt_raw_decoder(void);

// Stop the decode running in `decode` at its next check, from any thread
// (LibRaw's cancel flag, which its decoders check inside their loops).
void dt_raw_interrupt(DtRawDecode *decode);

// Decode to 16-bit linear camera-native RGB (no white balance, gamma 1.0,
// PPG demosaic), upright: LibRaw applies the camera's orientation. On success
// `decode` holds the pixels and *width / *height / *channels their geometry.
// `cancel` (may be NULL) is a flag that stops the decode between its stages
// once nonzero; `threads` (0 for LibRaw's own) sizes OpenMP's team.
int32_t dt_raw_decode(
    const DtRawPathChar *path,
    DtRawMetadata *meta,
    DtRawDecode *decode,
    int32_t *width,
    int32_t *height,
    int32_t *channels,
    const uint8_t *cancel,
    int32_t threads);

// Copy the interleaved pixels into `dest`, which holds `count` u16 — exactly
// width * height * channels.
int32_t dt_raw_copy_pixels(DtRawDecode *decode, uint16_t *dest, size_t count);

// Release a decode. NULL is a no-op.
void dt_raw_decode_free(DtRawDecode *decode);

// An embedded preview, held until dt_raw_preview_free.
typedef struct DtRawPreview DtRawPreview;

// What a DtRawPreview's bytes are.
#define DT_RAW_PREVIEW_JPEG 1 // a JPEG file, still compressed
#define DT_RAW_PREVIEW_RGB8 2 // interleaved 8-bit RGB, width * height * 3

// The embedded preview best suited to a thumbnail `min_size` pixels on its
// long edge: the smallest at least that big, else the largest, letterboxed
// ones last. Reads only that preview. JPEG XL and HEVC previews are skipped;
// LIBRAW_NO_THUMBNAIL when none is left.
//
// `width` and `height` may be 0 for a JPEG whose container doesn't record
// its size. `flip` is LibRaw's orientation still to apply (0 upright,
// 3 = 180°, 5 = 90° CCW, 6 = 90° CW). `data` lives until dt_raw_preview_free.
int32_t dt_raw_preview(
    const DtRawPathChar *path,
    int32_t min_size,
    DtRawPreview **out,
    int32_t *format,
    int32_t *width,
    int32_t *height,
    int32_t *flip,
    const uint8_t **data,
    size_t *len);

// Release a preview. NULL is a no-op.
void dt_raw_preview_free(DtRawPreview *preview);

// Render the raw data small, for files with no usable preview: half size
// (no demosaic), camera white balance, sRGB, 8-bit, upright. Much faster than
// dt_raw_decode but it still reads every raw pixel. Copy the pixels out with
// dt_raw_copy_pixels8; release with dt_raw_decode_free. `cancel` and
// `threads` as for dt_raw_decode.
int32_t dt_raw_decode_half(
    const DtRawPathChar *path,
    DtRawDecode *decode,
    int32_t *width,
    int32_t *height,
    const uint8_t *cancel,
    int32_t threads);

// Copy a dt_raw_decode_half result into `dest`, which holds `count` bytes:
// exactly width * height * 3.
int32_t dt_raw_copy_pixels8(DtRawDecode *decode, uint8_t *dest, size_t count);

// A static description of a LibRaw error code.
const char *dt_raw_strerror(int32_t code);

// The camera's name for a DNG's UniqueCameraModel and Adobe's profile names
// ("Canon EOS R5 Adobe Standard.dcp"), into `out` (cut to `capacity` bytes,
// NUL-terminated): the file's own UniqueCameraModel (LibRaw reads it only
// from a DNG), else "<make> <model>" without a make the model already starts
// with (case-insensitive), or whichever is there; each trimmed.
void dt_raw_camera_name(const char *make, const char *model, const char *unique, char *out,
                        size_t capacity);

// How many threads an OpenMP parallel region ran on just now, on the calling
// thread's team: 1 in a build without OpenMP (see build.rs for which have
// it). For the test that a thread LibRaw decoded on can exit.
int32_t dt_raw_parallel_threads(void);

#ifdef __cplusplus
} // extern "C"
#endif
