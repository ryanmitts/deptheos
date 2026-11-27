#include "dng_c_api.h"
#include "guard.h"
#include "dng_bridge.h"
#include "dng_bridge_testing.h"

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <optional>
#include <utility>

// The Rust declarations (src/lib.rs) are written by hand and pin the same
// offsets: a changed field changes both. Offsets assume 64-bit pointers.
#if INTPTR_MAX == INT64_MAX
static_assert(sizeof(DtDngRenderParams) == 1168, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, camera_to_prophoto) == 0, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, camera_white) == 36, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, black_level) == 48, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, baseline_exposure) == 56, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, shadow_scale) == 64, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, hue_sat_dims) == 72, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, hue_sat_encoding) == 84, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, hue_sat_data) == 88, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, hue_sat_count) == 96, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, look_table_dims) == 104, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, look_table_encoding) == 116, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, look_table_data) == 120, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, look_table_count) == 128, "DtDngRenderParams layout changed");
static_assert(offsetof(DtDngRenderParams, tone_curve) == 136, "DtDngRenderParams layout changed");
static_assert(sizeof(DtDngTags) == 72, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, white) == 48, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, user_crop) == 56, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, orientation) == 64, "DtDngTags layout changed");
static_assert(sizeof(DtDngWhite) == 16, "DtDngWhite layout changed");
static_assert(offsetof(DtDngWhite, y) == 8, "DtDngWhite layout changed");
static_assert(offsetof(DtDngTags, fields) == 32, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, field_count) == 40, "DtDngTags layout changed");
static_assert(sizeof(DtDngField) == 24, "DtDngField layout changed");
static_assert(offsetof(DtDngField, tag) == 0, "DtDngField layout changed");
static_assert(offsetof(DtDngField, kind) == 2, "DtDngField layout changed");
static_assert(offsetof(DtDngField, count) == 4, "DtDngField layout changed");
static_assert(offsetof(DtDngField, text) == 8, "DtDngField layout changed");
static_assert(offsetof(DtDngField, numbers) == 16, "DtDngField layout changed");
static_assert(offsetof(DtDngTags, software) == 0, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, user_comment) == 8, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, xmp) == 16, "DtDngTags layout changed");
static_assert(offsetof(DtDngTags, capture_time) == 24, "DtDngTags layout changed");
#endif

// The decoded result owns both pixels and metadata; Rust holds this opaquely.
struct DtDngImage {
    DngReadResult result;
};

namespace {

DtDngImage *wrap(DngReadResult &&decoded) { return new DtDngImage{std::move(decoded)}; }

std::optional<DngWhiteXY> white_of(const DtDngWhite *white) {
    if (!white) return std::nullopt;
    return DngWhiteXY{white->x, white->y};
}

std::string fixed_str(const char *s, size_t n) { return std::string(s, strnlen(s, n)); }

// A RAW value as an EXIF rational's part: rounded down and clamped to
// uint32, or none for NaN, infinity or a negative, so a hostile file leaves
// the tag out instead of an out-of-range cast (undefined behavior).
std::optional<uint32_t> exif_part(double v) {
    if (!std::isfinite(v) || v < 0) {
        return std::nullopt;
    }
    return static_cast<uint32_t>(std::min(v, static_cast<double>(UINT32_MAX)));
}

// `value`/`den`, or the absent {0, 0} (the writer skips a zero denominator).
DngURational exif_rational(double value, uint32_t den) {
    const std::optional<uint32_t> num = exif_part(value);
    return num ? DngURational{*num, den} : DngURational{};
}

// The top-left 3x3 of a row-major matrix with `stride` columns. LibRaw's
// layouts are wider (color 4x3, forward 3x4, calibration 4x4); for a
// 3-color camera the rest is padding.
DngMatrix3x3 matrix3x3(const float *flat, int stride) {
    std::array<double, 9> m;
    for (int r = 0; r < 3; ++r) {
        for (int c = 0; c < 3; ++c) {
            m[r * 3 + c] = flat[r * stride + c];
        }
    }
    return m;
}

DngColorScienceInfo color_science_from(const DtRawMetadata &m) {
    DngColorScienceInfo cs;

    for (int i = 0; i < 2; ++i) {
        // LibRaw zero-fills dng_color for non-DNG files, and an all-zero
        // calibration trips the bridge's "non-identity" check.
        if (!m.dng_has_color_matrix[i]) {
            continue;
        }
        const uint32_t illuminant = m.dng_illuminant[i];
        DngMatrix3x3 mat = matrix3x3(m.dng_color_matrix[i], 3);
        if (i == 0) {
            cs.colorMatrix1 = mat;
            cs.calibrationIlluminant1 = illuminant;
        } else {
            cs.colorMatrix2 = mat;
            cs.calibrationIlluminant2 = illuminant;
        }
        if (m.dng_has_forward_matrix[i]) {
            (i == 0 ? cs.forwardMatrix1 : cs.forwardMatrix2) = matrix3x3(m.dng_forward_matrix[i], 4);
        }
        if (m.dng_has_calibration[i]) {
            DngMatrix3x3 cal = matrix3x3(m.dng_calibration[i], 4);
            bool nonzero = false;
            for (double v : *cal) {
                if (v != 0.0) {
                    nonzero = true;
                    break;
                }
            }
            if (nonzero) {
                (i == 0 ? cs.cameraCalibration1 : cs.cameraCalibration2) = cal;
            }
        }
    }

    // No DNG color matrices: LibRaw's Adobe table (cam_xyz, XYZ→camera at
    // D65) is single-illuminant color science.
    if (!cs.colorMatrix1 && !cs.colorMatrix2) {
        bool nonzero = false;
        for (int i = 0; i < 9; ++i) {
            if (m.cam_xyz[i] != 0.0f) {
                nonzero = true;
                break;
            }
        }
        if (nonzero) {
            cs.colorMatrix1 = matrix3x3(m.cam_xyz, 3);
            cs.calibrationIlluminant1 = 21; // EXIF D65
        }
    }

    const int asn_count = std::clamp(m.as_shot_neutral_count, 0, 4);
    if (asn_count > 0) {
        for (int i = 0; i < asn_count; ++i) {
            cs.asShotNeutral.push_back(m.as_shot_neutral[i]);
        }
    } else if (m.cam_mul[0] > 0 && m.cam_mul[1] > 0 && m.cam_mul[2] > 0) {
        // AsShotNeutral is camMul's inverse, normalized to a largest of 1.
        const double inv[3] = {1.0 / m.cam_mul[0], 1.0 / m.cam_mul[1], 1.0 / m.cam_mul[2]};
        const double max_inv = std::max(inv[0], std::max(inv[1], inv[2]));
        for (double v : inv) {
            cs.asShotNeutral.push_back(v / max_inv);
        }
    }

    const int ab_count = std::clamp(m.analog_balance_count, 0, 4);
    for (int i = 0; i < ab_count; ++i) {
        cs.analogBalance.push_back(m.analog_balance[i]);
    }
    if (m.has_baseline_exposure) {
        cs.baselineExposure = m.baseline_exposure;
    }
    if (m.has_linear_response_limit) {
        cs.linearResponseLimit = m.linear_response_limit;
    }

    return cs;
}

// LUTs, noise profile and opcodes stay empty: LibRaw sources carry none.
DngImageMetadata metadata_from_raw(const DtRawMetadata &m) {
    DngImageMetadata md;

    md.camera.make = fixed_str(m.make, sizeof m.make);
    md.camera.model = fixed_str(m.model, sizeof m.model);
    // LibRaw's name for the camera (dt_raw_camera_name in raw_c_api.h).
    md.camera.uniqueCameraModel = fixed_str(m.unique_camera_model, sizeof m.unique_camera_model);
    md.camera.software = fixed_str(m.software, sizeof m.software);
    md.camera.artist = fixed_str(m.artist, sizeof m.artist);

    md.lens.make = fixed_str(m.lens_make, sizeof m.lens_make);
    md.lens.model = fixed_str(m.lens_model, sizeof m.lens_model);
    // A focal length of 0 is unknown; without both (and with no apertures)
    // the writer leaves the lens's specification out.
    const auto focal = [](float f) { return f > 0 ? exif_rational(f, 1) : DngURational{}; };
    md.lens.info = {focal(m.lens_min_focal), focal(m.lens_max_focal), {0, 0}, {0, 0}};
    md.lens.focalLengthIn35mm = static_cast<uint32_t>(std::max(m.focal_length_in_35mm, 0));

    // EXIF ExposureTime as a rational: >=1s -> seconds/1, <1s -> 1/round(1/t).
    // NaN fails both tests and leaves it out.
    const double shutter = m.shutter;
    if (shutter >= 1.0) {
        md.exposure.exposureTime = exif_rational(shutter + 0.5, 1);
    } else if (shutter > 0) {
        const std::optional<uint32_t> den = exif_part(1.0 / shutter + 0.5);
        if (den) {
            md.exposure.exposureTime = {1, *den};
        }
    }
    // FNumber and FocalLength keep one decimal: f/2.8 -> 28/10, 50mm -> 500/10.
    if (m.aperture > 0) {
        md.exposure.fnumber = exif_rational(static_cast<double>(m.aperture) * 10, 10);
    }
    if (m.focal_len > 0) {
        md.exposure.focalLength = exif_rational(static_cast<double>(m.focal_len) * 10, 10);
    }
    md.exposure.isoSpeed = {exif_part(m.iso_speed).value_or(0), 0, 0};

    // LibRaw's pixels are already upright (user_flip at its default), so the
    // camera's orientation would rotate them twice.
    md.geometry.orientation = 1;
    md.geometry.channels = m.raw_colors > 0 ? std::min(m.raw_colors, 3) : 3;
    md.geometry.useJXL = false;

    md.colorScience = color_science_from(m);

    return md;
}

} // namespace

// Every entry that reaches the SDK runs through one `guarded` (guard.h), so
// no exception crosses into Rust. One that makes a deptheos_host initializes
// the SDK inside its guard; the others call dt_dng_initialize first. The
// test hooks keep their own failure values and take no status.

extern "C" {

void dt_dng_initialize(void) {
    // A throw leaves the once flag unset, so the next entry point retries.
    guarded<bool>("dng_initialize", nullptr, [] {
        deptheos_dng_initialize();
        return true;
    });
}

void dt_dng_set_log_callback(DtDngLogCallback callback) {
    deptheos_set_dng_log_callback(callback);
}

#if DEPTHEOS_TEST_HOOKS
void dt_dng_set_reference_reads(int32_t on) { deptheos_dng_set_reference_reads(on != 0); }

void dt_dng_last_read_path(int32_t *stage1, int32_t *deferred_black_level) {
    const DngReadPath path = deptheos_dng_last_read_path();
    *stage1 = path.stage1;
    *deferred_black_level = path.deferredBlackLevel ? 1 : 0;
}

size_t dt_dng_encode_lossless_jpeg(const uint16_t *samples, uint32_t rows, uint32_t cols,
                                   uint32_t channels, uint32_t bits, uint8_t *out,
                                   size_t capacity) {
    dt_dng_initialize();
    return guarded<size_t>("dng_encode_lossless_jpeg", nullptr, [&]() -> size_t {
        const std::vector<uint8_t> stream =
            deptheos_dng_encode_lossless_jpeg(samples, rows, cols, channels, bits);
        if (stream.size() > capacity) return 0;
        std::memcpy(out, stream.data(), stream.size());
        return stream.size();
    });
}

int32_t dt_dng_compare_lossless_jpeg(const uint8_t *data, size_t size, size_t samples) {
    dt_dng_initialize();
    return guarded_or<int32_t>(-3, "dng_compare_lossless_jpeg", nullptr, nullptr, [&] {
        return deptheos_dng_compare_lossless_jpeg(data, size, samples);
    });
}

int32_t dt_dng_render_file_srgb(const char *path, const char *dcp_path, uint32_t x, uint32_t y,
                                uint32_t w, uint32_t h, uint16_t *out, size_t capacity) {
    if (!path || !out) return 0;
    return guarded<int32_t>("dng_render_file_srgb", nullptr, [&]() -> int32_t {
        const std::vector<uint16_t> pixels =
            deptheos_dng_render_file_srgb(path, dcp_path ? dcp_path : "", x, y, w, h);
        if (pixels.size() != capacity) return 0;
        std::memcpy(out, pixels.data(), capacity * sizeof(uint16_t));
        return 1;
    });
}

int32_t dt_dng_frame_of(const char *path, double *user_crop, uint32_t *orientation) {
    if (!path || !user_crop || !orientation) return 0;
    return guarded<int32_t>("dng_frame_of", nullptr, [&]() -> int32_t {
        const DngFrameTags frame = deptheos_dng_frame_of(path);
        std::memcpy(user_crop, frame.userCrop, sizeof frame.userCrop);
        *orientation = frame.orientation;
        return 1;
    });
}
#endif // DEPTHEOS_TEST_HOOKS

DtDngImage *dt_dng_read_into(const char *path, int32_t bypass_cache, uint64_t max_pixels,
                             DtDngPixelAlloc alloc, void *ctx, uint32_t threads,
                             const uint8_t *cancel, DtStatus *status) {
    if (!path || !alloc) {
        return nullptr;
    }
    return guarded<DtDngImage *>("read_dng", path, status, [&] {
        return wrap(deptheos_dng_read_into(path, bypass_cache != 0, max_pixels,
                                           [&](size_t count) { return alloc(ctx, count); },
                                           threads, cancel));
    });
}

DtDngImage *dt_dng_read_metadata(const char *path, DtStatus *status) {
    if (!path) {
        return nullptr;
    }
    return guarded<DtDngImage *>("read_dng_metadata", status,
                                 [&] { return wrap(deptheos_dng_read_metadata(path)); });
}

int32_t dt_dng_width(const DtDngImage *img) { return img ? img->result.width : 0; }
int32_t dt_dng_height(const DtDngImage *img) { return img ? img->result.height : 0; }
int32_t dt_dng_channels(const DtDngImage *img) { return img ? img->result.channels : 0; }

int64_t dt_dng_pixel_count(const DtDngImage *img) {
    return img ? static_cast<int64_t>(img->result.pixels.size()) : 0;
}

const uint16_t *dt_dng_pixels(const DtDngImage *img) {
    return (img && !img->result.pixels.empty()) ? img->result.pixels.data() : nullptr;
}

// Geometry the SDK can render or write without a null Stage3 image (a
// metadata-only handle has none) or an over-read (the writer forces 3
// planes for any channels != 1).
static bool valid_pixel_geometry(size_t pixel_count, int32_t w, int32_t h, int32_t ch) {
    if (w <= 0 || h <= 0 || (ch != 1 && ch != 3)) {
        return false;
    }
    return pixel_count == static_cast<size_t>(w) * static_cast<size_t>(h) * static_cast<size_t>(ch);
}

DtDngImage *dt_dng_render_linear_to_srgb(
    const DtDngImage *meta_source,
    const uint16_t *pixels,
    size_t pixel_count,
    int32_t width,
    int32_t height,
    int32_t channels,
    const DtDngWhite *white,
    double exposure,
    DtStatus *status) {
    if (!meta_source || !pixels || !valid_pixel_geometry(pixel_count, width, height, channels)) {
        return nullptr;
    }
    return guarded<DtDngImage *>("render_to_srgb", status, [&] {
        return wrap(deptheos_render_to_srgb(pixels, pixel_count, width, height, channels,
                                            meta_source->result.metadata, white_of(white),
                                            exposure));
    });
}

DtDngImage *dt_dng_from_raw_metadata(const DtRawMetadata *meta, DtStatus *status) {
    if (!meta) {
        return nullptr;
    }
    dt_dng_initialize();
    return guarded<DtDngImage *>("dng_from_raw_metadata", status, [&] {
        DngReadResult result;
        result.metadata = metadata_from_raw(*meta);
        return new DtDngImage{std::move(result)};
    });
}

int32_t dt_dng_apply_dcp(DtDngImage *img, const char *dcp_path, DtStatus *status) {
    if (!img || !dcp_path) {
        return 0;
    }
    dt_dng_initialize();
    return guarded<int32_t>("apply_dcp", dcp_path, status, [&] {
        deptheos_apply_dcp(img->result.metadata, dcp_path);
        return 1;
    });
}

// Each field with at most 16 values (as many as any copied tag has); an
// unknown kind, or one without values, is left out.
static std::vector<DngSourceTag> source_tags(const DtDngField *fields, size_t count) {
    std::vector<DngSourceTag> out;
    for (size_t i = 0; fields && i < count; ++i) {
        const DtDngField &f = fields[i];
        DngSourceTag tag;
        tag.tag = f.tag;
        size_t per_value = 0;
        switch (f.kind) {
        case DT_DNG_FIELD_TEXT:
            tag.text = f.text ? f.text : "";
            out.push_back(std::move(tag));
            continue;
        case DT_DNG_FIELD_INTEGERS:
            tag.kind = DngSourceTag::Integers;
            per_value = 1;
            break;
        case DT_DNG_FIELD_RATIONALS:
            tag.kind = DngSourceTag::Rationals;
            per_value = 2;
            break;
        case DT_DNG_FIELD_SRATIONALS:
            tag.kind = DngSourceTag::SRationals;
            per_value = 2;
            break;
        default:
            continue;
        }
        if (!f.numbers || f.count == 0) continue;
        const size_t n = std::min<size_t>(f.count, 16) * per_value;
        tag.numbers.assign(f.numbers, f.numbers + n);
        out.push_back(std::move(tag));
    }
    return out;
}

int32_t dt_dng_write(
    const DtDngImage *meta_source,
    const char *out_path,
    const uint16_t *pixels,
    size_t pixel_count,
    int32_t width,
    int32_t height,
    int32_t channels,
    const DtDngTags *tags,
    DtStatus *status) {
    if (!meta_source || !out_path || !pixels
        || !valid_pixel_geometry(pixel_count, width, height, channels)) {
        return 0;
    }
    return guarded<int32_t>("write_dng", out_path, status, [&] {
        if (!tags) {
            deptheos_write_dng(out_path, meta_source->result.metadata, pixels, pixel_count, width,
                               height, channels, DngWriteKind::ColorReference);
            return 1;
        }
        DngImageMetadata md = meta_source->result.metadata;
        if (tags->software && *tags->software) {
            md.camera.software = tags->software;
        }
        // LibRaw-built metadata carries no capture time.
        if (md.dates.original.empty() && tags->capture_time && *tags->capture_time) {
            md.dates.original = tags->capture_time;
        }
        md.stack.userComment = tags->user_comment ? tags->user_comment : "";
        md.stack.xmp = tags->xmp ? tags->xmp : "";
        md.stack.sourceTags = source_tags(tags->fields, tags->field_count);
        if (tags->white) {
            if (auto neutral = deptheos_neutral_for_white(md, *white_of(tags->white))) {
                md.colorScience.asShotNeutral = std::move(*neutral);
            }
        }
        if (tags->orientation < 1 || tags->orientation > 8) {
            throw std::invalid_argument("write_dng: Orientation is not 1 to 8");
        }
        md.geometry.orientation = tags->orientation;
        if (tags->user_crop) {
            const uint32_t *c = tags->user_crop;
            if (c[2] == 0 || c[3] == 0 || uint64_t(c[0]) + c[2] > uint64_t(width) ||
                uint64_t(c[1]) + c[3] > uint64_t(height)) {
                throw std::invalid_argument("write_dng: the user crop is not inside the picture");
            }
            md.geometry.userCrop = std::array<uint32_t, 4>{c[0], c[1], c[2], c[3]};
        }
        deptheos_write_dng(out_path, md, pixels, pixel_count, width, height, channels,
                           DngWriteKind::Picture);
        return 1;
    });
}

void dt_dng_free(DtDngImage *img) { delete img; }

DtDngRenderParams *dt_dng_compute_render_params(const DtDngImage *meta_source,
                                                const DtDngWhite *white,
                                                DtStatus *status) {
    if (!meta_source) {
        return nullptr;
    }
    return guarded<DtDngRenderParams *>("compute_render_params", status, [&] {
        return deptheos_compute_render_params(meta_source->result.metadata, white_of(white));
    });
}

int32_t dt_dng_as_shot_white(const DtDngImage *meta_source, DtDngWhite *out, int32_t *has,
                             DtStatus *status) {
    if (!meta_source || !out || !has) {
        return 0;
    }
    return guarded<int32_t>("as_shot_white", status, [&] {
        const std::optional<DngWhiteXY> white =
            deptheos_as_shot_white(meta_source->result.metadata);
        *has = white ? 1 : 0;
        if (white) *out = DtDngWhite{white->x, white->y};
        return 1;
    });
}

int32_t dt_dng_white_for_neutral(const DtDngImage *meta_source, const double *neutral,
                                 DtDngWhite *out, int32_t *has, DtStatus *status) {
    if (!meta_source || !neutral || !out || !has) {
        return 0;
    }
    return guarded<int32_t>("white_for_neutral", status, [&] {
        const std::optional<DngWhiteXY> white =
            deptheos_white_for_neutral(meta_source->result.metadata, neutral);
        *has = white ? 1 : 0;
        if (white) *out = DtDngWhite{white->x, white->y};
        return 1;
    });
}

int32_t dt_dng_temperature_of(DtDngWhite white, double *kelvin, double *tint) {
    if (!kelvin || !tint) {
        return 0;
    }
    return guarded<int32_t>("temperature_of", nullptr, [&] {
        deptheos_temperature_of(DngWhiteXY{white.x, white.y}, *kelvin, *tint);
        return 1;
    });
}

int32_t dt_dng_temperature_white(double kelvin, double tint, DtDngWhite *out) {
    if (!out) {
        return 0;
    }
    return guarded<int32_t>("temperature_white", nullptr, [&] {
        const DngWhiteXY white = deptheos_temperature_white(kelvin, tint);
        *out = DtDngWhite{white.x, white.y};
        return 1;
    });
}

void dt_dng_render_params_free(DtDngRenderParams *params) {
    if (!params) {
        return;
    }
    delete[] params->hue_sat_data;
    delete[] params->look_table_data;
    delete params;
}

} // extern "C"
