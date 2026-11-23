// Plain-C reduced JPEG decoding over the vendored libjpeg the DNG SDK
// already links (jpeg_c_api.cpp).

#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// A decoded picture, owned by the C side until dt_jpeg_image_free.
typedef struct DtJpegImage DtJpegImage;

// Decode the JPEG in `data` (`size` bytes) as RGB8 at the smallest n/8 of
// its stored size (n from 1 to 8) whose long side is at least `min_side`,
// or whole when that is the only one. NULL for a JPEG it can't read, or a
// CMYK one.
DtJpegImage *dt_jpeg_decode_scaled(const uint8_t *data, size_t size, uint32_t min_side);

// Its pixels, `width` * `height` * 3 bytes, rows top to bottom as stored.
const uint8_t *dt_jpeg_image_rgb(const DtJpegImage *image);
uint32_t dt_jpeg_image_width(const DtJpegImage *image);
uint32_t dt_jpeg_image_height(const DtJpegImage *image);
void dt_jpeg_image_free(DtJpegImage *image);

#ifdef __cplusplus
}
#endif
