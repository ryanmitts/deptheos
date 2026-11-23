// Plain-C lossless JPEG XL codec for interleaved 16-bit RGB and 16-bit gray,
// over the vendored libjxl the DNG SDK already links. Projects store their
// renders with it: at effort 1 (libjxl's fast-lossless path) it encodes several
// times faster than PNG16 and to a markedly smaller file.

#pragma once

#include <stdint.h>
#include <stddef.h>

// DtStatus: why a call failed, which each call below says in its last
// argument (may be NULL).
#include "dt_status.h"

#ifdef __cplusplus
extern "C" {
#endif

// Where an encode writes: `write` takes `size` bytes at the current
// position, `seek` moves it (back, to fill in what is known last). Each
// returns 1 on success, 0 to stop the encode.
typedef int32_t (*DtJxlWrite)(void *ctx, const uint8_t *data, size_t size);
typedef int32_t (*DtJxlSeek)(void *ctx, uint64_t position);

// Encode `width`x`height` samples losslessly at `effort` (1-9) through
// `write` and `seek` with `ctx`, a part at a time as libjxl finishes it,
// reading the samples a group at a time: neither the picture nor the
// file is copied whole. Interleaved RGB u16 (`channels` 3, `bits` 16) or
// gray u16 (`channels` 1, `bits` 16); `sample_count` is
// width * height * channels. `linear` tags the color encoding as linear
// rather than sRGB transfer. Runs on libjxl's thread pool of `threads`
// (0: one per core), stopped once `cancel` (may be NULL) is nonzero.
// Returns 1 on success, 0 on failure, cancel or a failed write.
int32_t dt_jxl_encode(const void *samples, size_t sample_count, uint32_t width, uint32_t height,
                      int32_t channels, int32_t bits, int32_t effort, int32_t linear,
                      int32_t threads, const uint8_t *cancel, void *ctx, DtJxlWrite write,
                      DtJxlSeek seek, DtStatus *status);

// Read the image size and channel count from a codestream's header.
// Returns 1 on success, 0 on failure (dng_error_bad_format for data that
// is no codestream).
int32_t dt_jxl_info(const uint8_t *data, size_t size, uint32_t *width, uint32_t *height,
                    uint32_t *channels, DtStatus *status);

// Decode a codestream into `out` in the layout dt_jxl_encode takes
// (`channels` 3 or 1, with `bits` 16); `sample_count` must be
// width * height * channels. At most `threads` run (0: one per core);
// once `cancel` (may be NULL) is nonzero the decode stops within a stage.
// Returns 1 on success, 0 on failure (dng_error_bad_format for a
// codestream libjxl can't decode) or cancel (dng_error_user_canceled).
int32_t dt_jxl_decode(const uint8_t *data, size_t size, void *out, size_t sample_count,
                      int32_t channels, int32_t bits, int32_t threads, const uint8_t *cancel,
                      DtStatus *status);

#ifdef __cplusplus
} // extern "C"
#endif
