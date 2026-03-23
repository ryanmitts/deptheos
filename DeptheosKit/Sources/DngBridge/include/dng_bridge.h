//
//  dng_bridge.h
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//
#pragma once

#include <stdint.h>
#include <string>
#include <vector>
#include <optional>

typedef enum {
    DNG_LOG_TRACE = 0,
    DNG_LOG_DEBUG = 1,
    DNG_LOG_INFO = 2,
    DNG_LOG_WARN = 3,
    DNG_LOG_ERROR = 4
} DngLogLevel;

typedef void (*DngLogCallback)(DngLogLevel level, const char *message);

void deptheos_dng_initialize(void);
void deptheos_set_dng_log_callback(DngLogCallback callback);

void dng_log(DngLogLevel level, const char *format, ...);

struct DngURational {
    uint32_t num = 0;
    uint32_t den = 0;
};

struct DngSRational {
    int32_t num = 0;
    int32_t den = 0;
};

struct DngMatrix3x3 {
    std::vector<double> values;  // 9 elements, row-major
    bool isValid = false;
};

struct DngHueSatDims {
    uint32_t hueDivisions = 0;
    uint32_t satDivisions = 0;
    uint32_t valDivisions = 0;
};

struct DngCameraInfo {
    std::string make, model, uniqueCameraModel;
    std::string serialNumber, software, artist, copyright;
};

struct DngLensInfo {
    std::string make, model, serialNumber;
    std::vector<DngURational> info;  // 4 elements
    uint32_t focalLengthIn35mm = 0;
};

struct DngExposureInfo {
    DngURational exposureTime, fnumber, focalLength, maxApertureValue;
    DngSRational exposureBias;
    std::vector<uint32_t> isoSpeed;  // 3 elements
    uint32_t program = 0, mode = 0, meteringMode = 0;
    uint32_t flash = 0, whiteBalance = 0, sceneCaptureType = 0;
    DngURational focalPlaneXRes, focalPlaneYRes;
    uint32_t focalPlaneResUnit = 0;
};

struct DngDateInfo {
    std::string original, digitized;
};

struct DngGPSInfo {
    uint32_t versionId = 0;
    std::string latitudeRef;
    std::vector<DngURational> latitude;   // 3 elements
    std::string longitudeRef;
    std::vector<DngURational> longitude;  // 3 elements
    uint32_t altitudeRef = 0;
    DngURational altitude;
};

struct DngGeometryInfo {
    uint32_t orientation = 0;
    int32_t channels = 0;
    bool useJXL = false;
};

struct DngColorScienceInfo {
    DngMatrix3x3 colorMatrix1, colorMatrix2;
    DngMatrix3x3 forwardMatrix1, forwardMatrix2;
    DngMatrix3x3 cameraCalibration1, cameraCalibration2;
    uint32_t calibrationIlluminant1 = 0, calibrationIlluminant2 = 0;
    std::string cameraCalibrationSig, profileCalibrationSig, profileName;
    std::vector<double> asShotNeutral;   // up to 4
    std::vector<double> analogBalance;   // up to 4
    std::optional<double> baselineExposure;
    std::optional<double> baselineNoise;
    std::optional<double> baselineSharpness;
    std::optional<double> shadowScale;
    std::optional<double> linearResponseLimit;
};

struct DngLUTData {
    std::optional<DngHueSatDims> hueSatMapDims1;
    std::vector<float> hueSatMapData1;
    std::optional<DngHueSatDims> hueSatMapDims2;
    std::vector<float> hueSatMapData2;
    uint32_t hueSatMapEncoding = 0;
    std::optional<DngHueSatDims> lookTableDims;
    std::vector<float> lookTableData;
    uint32_t lookTableEncoding = 0;
    std::vector<float> profileToneCurve;  // XY knot pairs
};

struct DngNoiseProfile {
    std::vector<double> scale;
    std::vector<double> offset;
};

struct DngOpcodeData {
    std::vector<uint8_t> opcodeList2;
    std::vector<uint8_t> opcodeList3;
};

// MARK: - Main Structs

struct DngImageMetadata {
    DngCameraInfo camera;
    DngLensInfo lens;
    DngExposureInfo exposure;
    DngDateInfo dates;
    DngGPSInfo gps;
    DngGeometryInfo geometry;
    DngColorScienceInfo colorScience;
    DngLUTData luts;
    DngNoiseProfile noiseProfile;
    DngOpcodeData opcodes;
};

struct DngRenderParams {
    std::vector<float> cameraToProPhoto;  // 9 elements
    std::vector<float> cameraWhite;       // 3 elements
    bool wbBakedIn = false;
    double blackLevel = 0;
    double baselineExposure = 0;
    double shadowScale = 1;
    std::optional<DngHueSatDims> hueSatMapDims;
    std::vector<float> hueSatMapData;
    std::optional<DngHueSatDims> lookTableDims;
    std::vector<float> lookTableData;
    std::vector<float> toneCurve;  // 257 elements
};

struct DngReadResult {
    std::vector<uint16_t> pixels;
    int32_t width = 0;
    int32_t height = 0;
    int32_t channels = 0;
    DngImageMetadata metadata;
};

struct DngDcpResult {
    DngImageMetadata profile;
    double baselineExposureOffset = 0;
};

// Type aliases for Swift C++ interop
using CxxFloatVector = std::vector<float>;
using CxxDoubleVector = std::vector<double>;
using CxxUInt32Vector = std::vector<uint32_t>;
using CxxUInt8Vector = std::vector<uint8_t>;
using CxxUInt16Vector = std::vector<uint16_t>;
using CxxURationalVector = std::vector<DngURational>;
using CxxSRationalVector = std::vector<DngSRational>;
using CxxOptionalDouble = std::optional<double>;
using CxxOptionalHueSatDims = std::optional<DngHueSatDims>;
using CxxOptionalDngReadResult = std::optional<DngReadResult>;
using CxxOptionalDngRenderParams = std::optional<DngRenderParams>;
using CxxOptionalDngDcpResult = std::optional<DngDcpResult>;
using CxxOptionalUInt8Vector = std::optional<std::vector<uint8_t>>;

inline CxxOptionalDouble makeOptionalDouble(double value) {
    return CxxOptionalDouble(value);
}

inline CxxOptionalHueSatDims makeOptionalHueSatDims(DngHueSatDims value) {
    return CxxOptionalHueSatDims(value);
}

// Read
std::optional<DngReadResult> deptheos_dng_read(const std::string &path);
std::optional<DngReadResult> deptheos_dng_read_from_bytes(const uint8_t *bytes, size_t byte_count);

// Write
bool deptheos_write_dng(const std::string &output_path,
                        const DngImageMetadata &meta,
                        const uint16_t *pixels, size_t pixel_count,
                        int32_t width, int32_t height, int32_t channels);
std::optional<std::vector<uint8_t>> deptheos_write_dng_to_bytes(
        const DngImageMetadata &meta,
        const uint16_t *pixels, size_t pixel_count,
        int32_t width, int32_t height, int32_t channels);

// Render
std::optional<DngReadResult> deptheos_render_to_srgb(
        const uint16_t *pixels, size_t pixel_count,
        int32_t width, int32_t height, int32_t channels,
        const DngImageMetadata &meta);

// Metadata
std::optional<DngRenderParams> deptheos_compute_render_params(const DngImageMetadata &meta);
std::optional<DngDcpResult> deptheos_load_dcp(const std::string &dcp_path);

// Validation
bool deptheos_validate_dng(const std::string &path);

// --- Two-phase API: decode → write into caller buffer ---

/// Handle keeping decoded DNG state alive between open and write phases.
struct DngDecodeHandle {
    DngReadResult metadata;    // metadata + dimensions (pixels vector unused/empty)
    void *_internal = nullptr; // opaque: holds host + negative + crop info
};

using CxxOptionalDngDecodeHandle = std::optional<DngDecodeHandle>;

/// Phase 1: Decode and extract metadata. Pixel data held internally.
std::optional<DngDecodeHandle> deptheos_open_dng(const std::string &path);
std::optional<DngDecodeHandle> deptheos_open_dng_from_bytes(const uint8_t *bytes, size_t byte_count);

/// Phase 2a: Copy interleaved RGB16 pixels into caller's buffer.
void deptheos_copy_dng_pixels(DngDecodeHandle &handle, void *dest);

/// Phase 2b: Copy pixels directly into an RGBA16 buffer (stride-4 layout).
/// Writes RGB into positions 0-2 of each 4-uint16 group and fills alpha (0xFFFF).
/// rowBytes is the actual byte stride of the destination buffer.
void deptheos_copy_dng_pixels_rgba(DngDecodeHandle &handle, void *dest, int32_t rowBytes);

/// Phase 3: Free internal state.
void deptheos_close_dng(DngDecodeHandle &handle);
