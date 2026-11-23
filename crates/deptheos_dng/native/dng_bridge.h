// DngBridge: a value-type C++ API over the Adobe DNG SDK, with no SDK types.
// Its functions throw what the SDK throws; the C API (dng_c_api.cpp) is the
// barrier that catches it.
#pragma once

#include <stdint.h>
#include <array>
#include <functional>
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

// `level` is a DngLogLevel; int32_t so the C API can pass its callback through.
typedef void (*DngLogCallback)(int32_t level, const char *message);

void deptheos_dng_initialize(void);
void deptheos_set_dng_log_callback(DngLogCallback callback);

void dng_log(DngLogLevel level, const char *format, ...);

// The test hooks (reference reads and the like) are in dng_bridge_testing.h.

struct DngURational {
    uint32_t num = 0;
    uint32_t den = 0;
};

struct DngSRational {
    int32_t num = 0;
    int32_t den = 0;
};

// Row-major; none when the file has no such matrix.
using DngMatrix3x3 = std::optional<std::array<double, 9>>;

struct DngHueSatDims {
    uint32_t hueDivisions = 0;
    uint32_t satDivisions = 0;
    uint32_t valDivisions = 0;
};

// What a DNG says of the camera and lens. Nothing that says which body or
// lens took a frame, or where (serial numbers, GPS), is read or held, so no
// DNG written carries it (ADR-133).
struct DngCameraInfo {
    std::string make, model, uniqueCameraModel;
    std::string software, artist, copyright;
};

struct DngLensInfo {
    std::string make, model;
    std::array<DngURational, 4> info;
    uint32_t focalLengthIn35mm = 0;
};

struct DngExposureInfo {
    DngURational exposureTime, fnumber, focalLength, maxApertureValue;
    DngSRational exposureBias;
    std::array<uint32_t, 3> isoSpeed{};
    uint32_t program = 0, mode = 0, meteringMode = 0;
    uint32_t flash = 0, whiteBalance = 0, sceneCaptureType = 0;
    DngURational focalPlaneXRes, focalPlaneYRes;
    uint32_t focalPlaneResUnit = 0;
};

struct DngDateInfo {
    std::string original, digitized;
};

struct DngGeometryInfo {
    uint32_t orientation = 0;
    int32_t channels = 0;
    // On write, compress losslessly with JPEG XL; on read, the source was.
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

// The camera profile's tables and the profile settings the render reads
// with them.
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
    // DefaultBlackRender: 1 (None) turns the render's shadow ramp off.
    uint32_t defaultBlackRender = 0;
    // BaselineExposureOffset, stops, added to the negative's BaselineExposure.
    double baselineExposureOffset = 0;
};

struct DngNoiseProfile {
    std::vector<double> scale;
    std::vector<double> offset;
};

struct DngOpcodeData {
    std::vector<uint8_t> opcodeList2;
    // Opcode list 3 works in stage 3's coordinates, so it is kept only for
    // an image framed as that stage 3: `list3Width` × `list3Height`,
    // upright, uncropped. A write or render at another size leaves it out.
    std::vector<uint8_t> opcodeList3;
    int32_t list3Width = 0;
    int32_t list3Height = 0;
};

// A tag copied from the reference frame, written over what the rest of the
// metadata says for it (`apply_source_tags` in dng_write.cpp). Rationals
// are numerator, denominator pairs; a signed one's parts are int32 bits.
struct DngSourceTag {
    enum Kind { Text, Integers, Rationals, SRationals };
    uint16_t tag = 0;
    Kind kind = Text;
    std::string text;
    std::vector<uint32_t> numbers;
};

// What a stacked result adds on export; never read back from a file.
struct DngStackInfo {
    std::string userComment;  // the Exif UserComment, UTF-8
    std::string xmp;          // an XMP packet merged into the file's XMP
    std::vector<DngSourceTag> sourceTags; // the frame's own: an export's allowlist
};

// MARK: - Main Structs

struct DngImageMetadata {
    DngCameraInfo camera;
    DngLensInfo lens;
    DngExposureInfo exposure;
    DngDateInfo dates;
    DngGeometryInfo geometry;
    DngColorScienceInfo colorScience;
    DngLUTData luts;
    DngNoiseProfile noiseProfile;
    DngOpcodeData opcodes;
    DngStackInfo stack;
};


struct DngReadResult {
    std::vector<uint16_t> pixels;
    int32_t width = 0;
    int32_t height = 0;
    int32_t channels = 0;
    DngImageMetadata metadata;
};

// The calls below fail only by throwing, so guard.h can say why: besides
// the SDK's errors, a non-DNG is dng_error_bad_format, a floating-point DNG
// dng_error_not_yet_implemented, and no pixel buffer dng_error_memory.

// Read
// Metadata only (no pixels, channels = 0): parses the negative without reading its image.
DngReadResult deptheos_dng_read_metadata(const std::string &path);

// Called once with the sample count (width × height × channels); returns
// room for that many uint16_t, or nullptr to fail the read.
using DngPixelAlloc = std::function<uint16_t *(size_t count)>;
// Decodes to linear 16-bit camera RGB into the buffer `alloc` returns;
// `result.pixels` stays empty. A mosaic has no pixels (channels = 0) and
// never calls `alloc`. As dt_dng_read_into (dng_c_api.h), but a cancel
// throws dng_error_user_canceled and a too-large file DT_ERROR_TOO_LARGE.
DngReadResult deptheos_dng_read_into(const std::string &path, bool bypassCache,
                                     uint64_t maxPixels,
                                     const DngPixelAlloc &alloc, uint32_t threads,
                                     const uint8_t *cancel);

// What a write is for.
enum class DngWriteKind {
    // A picture: opcode list 3 goes with it only when its pixels are the
    // size the list applies to (DngOpcodeData).
    Picture,
    // A stand-in that carries `meta` alone (the Library's color reference):
    // opcode list 3 goes with it whatever its size, its size written as
    // OriginalDefaultCropSize, which a read takes back.
    ColorReference,
};

// A failed write can leave part of a file at `output_path`: callers write
// to a temporary path and move it into place (Rust's `io::write_replacing`).
void deptheos_write_dng(const std::string &output_path,
                        const DngImageMetadata &meta,
                        const uint16_t *pixels, size_t pixel_count,
                        int32_t width, int32_t height, int32_t channels,
                        DngWriteKind kind);

// Render: linear pixels with `meta`'s color science, rendered by the SDK to
// 16-bit sRGB (gray gamma 2.2 for one channel).
DngReadResult deptheos_render_to_srgb(
        const uint16_t *pixels, size_t pixel_count,
        int32_t width, int32_t height, int32_t channels,
        const DngImageMetadata &meta);

// Metadata
// The GPU render's parameters, in the C API's own struct; free with
// dt_dng_render_params_free.
struct DtDngRenderParams *deptheos_compute_render_params(const DngImageMetadata &meta);
// Replaces `meta`'s camera profile with the DCP's; camera calibration and
// as-shot neutral stay. On a throw (dng_error_bad_format for a non-DCP),
// `meta` is unchanged.
void deptheos_apply_dcp(DngImageMetadata &meta, const std::string &dcp_path);
