#pragma once

#include <stdint.h>
#include <string>
#include <vector>
#include <optional>

/// DNG-style color matrix for one illuminant.
struct RawDngColor {
    uint16_t illuminant = 0;
    std::vector<float> colorMatrix;     // 12 elements (4x3 row-major, XYZ→camera)
    std::vector<float> forwardMatrix;   // 12 elements (3x4 row-major, camera→XYZ)
    std::vector<float> calibration;     // 16 elements (4x4 row-major)
    bool hasColorMatrix = false;
    bool hasForwardMatrix = false;
    bool hasCalibration = false;
};

/// GPS info.
struct RawGPSInfo {
    std::vector<float> latitude;   // 3 elements: degrees, minutes, seconds
    std::vector<float> longitude;  // 3 elements
    float altitude = 0;
    char latref = 0;    // 'N' or 'S'
    char longref = 0;   // 'E' or 'W'
    char altref = 0;    // 0=above, 1=below sea level
    bool parsed = false;
};

/// Lens info.
struct RawLensInfo {
    std::string make, model, serial;
    float minFocal = 0;
    float maxFocal = 0;
    float maxApAtMinFocal = 0;
    float maxApAtMaxFocal = 0;
    uint16_t focalLengthIn35mm = 0;
};

/// Result of reading a raw file.
struct RawReadResult {
    std::vector<uint16_t> pixels;
    int32_t width = 0;
    int32_t height = 0;
    int32_t channels = 0;   // always 3 (RGB)
    int32_t bits = 0;       // always 16

    // Camera info
    std::string make, model, software, uniqueCameraModel, artist, description;
    int32_t orientation = 0;  // EXIF orientation (1-8)

    // Exposure
    float isoSpeed = 0;
    float shutter = 0;     // seconds
    float aperture = 0;    // f-number
    float focalLen = 0;    // mm
    int32_t timestamp = 0; // UNIX timestamp

    // White balance
    std::vector<float> camMul;   // 4 elements (R, G1, B, G2)
    std::vector<float> preMul;   // 4 elements (daylight WB)

    // Color science
    std::vector<float> camXYZ;   // 12 elements (4x3 camera→XYZ D65)
    std::vector<float> rgbCam;   // 12 elements (3x4 sRGB→camera)
    std::vector<RawDngColor> dngColor;  // 2 illuminants

    // Levels
    std::vector<uint32_t> cblack;      // 4 elements (per-channel black)
    uint32_t black = 0;                // overall black level
    uint32_t maximum = 0;              // white level
    std::vector<uint32_t> linearMax;   // 4 elements (per-channel white)

    // DNG-specific parsed levels
    std::vector<float> asShotNeutral;  // up to 4 (empty if not present)
    std::vector<float> analogBalance;  // up to 4 (empty if not present)
    std::optional<float> baselineExposure;
    std::optional<float> linearResponseLimit;

    // Number of raw colors (3 or 4)
    int32_t rawColors = 0;

    // Lens & GPS
    RawLensInfo lens;
    RawGPSInfo gps;
};

// Type aliases for Swift interop
using CxxRawDngColorVector = std::vector<RawDngColor>;
using CxxRawUInt8Vector = std::vector<uint8_t>;
using CxxRawOptionalFloat = std::optional<float>;
using CxxOptionalRawReadResult = std::optional<RawReadResult>;

// Read to sRGB 16-bit
std::optional<RawReadResult> deptheos_read_raw(const std::string &path);
std::optional<RawReadResult> deptheos_read_raw_from_bytes(const uint8_t *bytes, size_t byte_count);

// Read to linear 16-bit camera-native RGB
std::optional<RawReadResult> deptheos_read_raw_linear(const std::string &path);
std::optional<RawReadResult> deptheos_read_raw_linear_from_bytes(const uint8_t *bytes, size_t byte_count);

// --- Two-phase API: decode → write into caller buffer ---

/// Handle keeping decoded pixels alive between open and write phases.
struct RawDecodeHandle {
    RawReadResult metadata;   // metadata + dimensions (pixels vector unused/empty)
    void *_internal = nullptr; // opaque: libraw_processed_image_t*
};

using CxxOptionalRawDecodeHandle = std::optional<RawDecodeHandle>;

/// Phase 1: Decode and extract metadata. Pixel data held internally.
std::optional<RawDecodeHandle> deptheos_open_raw_linear(const std::string &path);
std::optional<RawDecodeHandle> deptheos_open_raw_linear_from_bytes(const uint8_t *bytes, size_t byte_count);

/// Phase 2: Copy interleaved RGB16 pixels into caller's buffer.
void deptheos_copy_raw_pixels(RawDecodeHandle &handle, void *dest);

/// Get a direct pointer to the decoded pixel data (valid until deptheos_close_raw).
void *deptheos_raw_pixel_data(RawDecodeHandle &handle);

/// Phase 3: Free internal pixel buffer.
void deptheos_close_raw(RawDecodeHandle &handle);
