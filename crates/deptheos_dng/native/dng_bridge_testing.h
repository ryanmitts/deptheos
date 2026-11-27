// Test hooks of the DngBridge, for tests that check the bridge's faster read
// paths against the SDK's. Not product API.
#pragma once

#include <stddef.h>
#include <stdint.h>
#include <string>
#include <vector>

// With `on`, this thread's reads take the SDK's path, not the bridge's.
void deptheos_dng_set_reference_reads(bool on);

// How this thread's last read of linear pixels took them.
struct DngReadPath {
    // A stage1_read value (dng_bridge_internal.h), or -1 before any read.
    int32_t stage1 = -1;
    // Whether the black level waited for the copy out.
    bool deferredBlackLevel = false;
};
DngReadPath deptheos_dng_last_read_path();

// The SDK's lossless JPEG encoding of `rows` x `cols` x `channels` samples
// of `bits` bits.
std::vector<uint8_t> deptheos_dng_encode_lossless_jpeg(const uint16_t *samples, uint32_t rows,
                                                       uint32_t cols, uint32_t channels,
                                                       uint32_t bits);

// The bridge's lossless JPEG decoder against the SDK's on one stream of
// `samples` samples: 1 when both decode it alike, 0 when the bridge's
// declines it, -1 when it decodes a stream the SDK rejects, -2 when the two
// differ.
int deptheos_dng_compare_lossless_jpeg(const uint8_t *data, size_t size, size_t samples);

// `w` x `h` RGB pixels at (`x`, `y`) of the default crop, rendered to 16-bit
// sRGB by the SDK alone: the GPU render's reference. A non-empty `dcp_path`
// replaces the file's profile. Opcode list 3 is left out, as the bridge's
// linear pixels leave it out. Throws for a file not upright or not RGB.
std::vector<uint16_t> deptheos_dng_render_file_srgb(const std::string &path,
                                                    const std::string &dcp_path, uint32_t x,
                                                    uint32_t y, uint32_t w, uint32_t h);

// A file's DefaultUserCrop (top, left, bottom, right, relative to its
// default crop) and Orientation tag (EXIF's 1 to 8), as the SDK parses them.
struct DngFrameTags {
    double userCrop[4] = {0, 0, 1, 1};
    uint32_t orientation = 1;
};
DngFrameTags deptheos_dng_frame_of(const std::string &path);
