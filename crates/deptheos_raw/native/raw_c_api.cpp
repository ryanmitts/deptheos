#include "raw_c_api.h"

#include "libraw/libraw.h"

#include <algorithm>
#include <cctype>
#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstring>
#include <memory>
#include <new>
#include <string>
#include <vector>

#ifdef _OPENMP
#include <omp.h>
#endif

// What this file relies on in LibRaw, to re-check on a LibRaw update:
// - color.dng_levels.parsedfields is never set; an absent BaselineExposure
//   keeps its -999 sentinel (read_metadata).
// - LibRaw_internal_thumbnail_formats and the thumbs_list fields the
//   preview picker reads (usable_preview, dt_raw_preview).
// - color.cam_xyz is 4x3 and dng_color[] holds two illuminants.
static_assert(LIBRAW_VERSION == LIBRAW_MAKE_VERSION(0, 23, 0),
              "LibRaw updated: re-check raw_c_api.cpp against it (see above)");

// The Rust declaration of DtRawMetadata (src/lib.rs) is written by hand and
// pins the same offsets. A changed field changes both lists.
static_assert(sizeof(DtRawMetadata) == 1080, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, make) == 0, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, model) == 64, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, software) == 128, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, unique_camera_model) == 192, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, artist) == 256, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, lens_make) == 320, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, lens_model) == 448, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, lens_min_focal) == 576, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, lens_max_focal) == 580, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, focal_length_in_35mm) == 584, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, iso_speed) == 588, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, shutter) == 592, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, aperture) == 596, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, focal_len) == 600, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, raw_colors) == 604, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, cam_mul) == 608, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, cam_xyz) == 624, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, as_shot_neutral) == 672, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, as_shot_neutral_count) == 688, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_illuminant) == 692, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_color_matrix) == 700, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_forward_matrix) == 796, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_calibration) == 892, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_has_color_matrix) == 1020, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_has_forward_matrix) == 1028, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, dng_has_calibration) == 1036, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, analog_balance) == 1044, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, analog_balance_count) == 1060, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, baseline_exposure) == 1064, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, has_baseline_exposure) == 1068, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, linear_response_limit) == 1072, "DtRawMetadata layout changed");
static_assert(offsetof(DtRawMetadata, has_linear_response_limit) == 1076, "DtRawMetadata layout changed");

// The `LibRaw` object is ≈ 750 KB, too big for a worker thread's stack
// (512 KB is common), so it always lives on the heap. Decoding itself
// needs little stack: a PPG decode of CR3/ORF/DNG/RAF fits in 64 KB.
struct DtRawDecode {
    LibRaw processor;
};

// A preview LibRaw unpacked: a JPEG points into the processor's buffer; a
// bitmap is converted to 8-bit RGB in `rgb`.
struct DtRawPreview {
    LibRaw processor;
    std::vector<uint8_t> rgb;
};

namespace {

// Runs `body`, turning a C++ exception into an error code so none crosses
// into Rust (unwinding out of an extern "C" function aborts the process).
template <typename Body>
int32_t guarded(Body body) {
    try {
        return body();
    } catch (const std::bad_alloc &) {
        return LIBRAW_UNSUFFICIENT_MEMORY;
    } catch (...) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
}

// Copy a fixed LibRaw char array (not necessarily NUL-terminated) into a
// fixed C buffer, always NUL-terminated.
template <size_t N, size_t M>
void copy_string(char (&dest)[N], const char (&src)[M]) {
    const size_t n = std::min(strnlen(src, M), N - 1);
    std::memcpy(dest, src, n);
    dest[n] = '\0';
}

// Copy a LibRaw matrix (row-major, so a plain memcpy flattens it) when any
// entry differs from `absent(row, col)`, LibRaw's value for a missing tag.
// Returns whether it did; `dest` is left zeroed otherwise.
template <size_t R, size_t C, typename Absent>
int32_t copy_if_present(float *dest, const float (&src)[R][C], Absent absent) {
    bool present = false;
    for (size_t r = 0; r < R; ++r) {
        for (size_t c = 0; c < C; ++c) {
            present |= src[r][c] != absent(r, c);
        }
    }
    if (present) {
        std::memcpy(dest, src, sizeof(src));
    }
    return present;
}

// The leading non-zero entries of a LibRaw DNG level array (zero = absent).
int32_t copy_levels(float (&dest)[4], const float (&src)[4]) {
    int32_t count = 0;
    while (count < 4 && src[count] != 0.0f) {
        dest[count] = src[count];
        ++count;
    }
    return count;
}

std::string trimmed(const char *s) {
    const std::string text(s);
    const size_t first = text.find_first_not_of(" \t");
    if (first == std::string::npos) return "";
    const size_t last = text.find_last_not_of(" \t");
    return text.substr(first, last - first + 1);
}

// The name the camera goes by in a DNG's UniqueCameraModel and in Adobe's
// profile names (dt_raw_camera_name).
std::string camera_name(const char *make_in, const char *model_in, const char *unique_in) {
    const std::string unique = trimmed(unique_in);
    if (!unique.empty()) return unique;
    const std::string make = trimmed(make_in);
    const std::string model = trimmed(model_in);
    if (make.empty()) return model;
    if (model.empty()) return make;
    const auto lower = [](std::string s) {
        for (char &c : s) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        return s;
    };
    if (lower(model).rfind(lower(make), 0) == 0) return model;
    return make + " " + model;
}

// `text` into a C buffer of `capacity` bytes, cut to fit and NUL-terminated.
void copy_text(const std::string &text, char *dest, size_t capacity) {
    const size_t n = std::min(text.size(), capacity - 1);
    std::memcpy(dest, text.data(), n);
    dest[n] = '\0';
}

void fill_metadata(DtRawMetadata *out, const libraw_data_t &d) {
    std::memset(out, 0, sizeof(DtRawMetadata));

    copy_string(out->make, d.idata.make);
    copy_string(out->model, d.idata.model);
    copy_string(out->software, d.idata.software);
    copy_string(out->unique_camera_model, d.color.UniqueCameraModel);
    copy_text(camera_name(out->make, out->model, out->unique_camera_model),
              out->unique_camera_model, sizeof out->unique_camera_model);
    copy_string(out->artist, d.other.artist);

    copy_string(out->lens_make, d.lens.LensMake);
    copy_string(out->lens_model, d.lens.Lens);
    out->lens_min_focal = d.lens.MinFocal;
    out->lens_max_focal = d.lens.MaxFocal;
    out->focal_length_in_35mm = d.lens.FocalLengthIn35mmFormat;

    out->iso_speed = d.other.iso_speed;
    out->shutter = d.other.shutter;
    out->aperture = d.other.aperture;
    out->focal_len = d.other.focal_len;
    out->raw_colors = d.idata.colors;

    std::copy_n(d.color.cam_mul, 4, out->cam_mul);
    static_assert(sizeof(out->cam_xyz) == sizeof(d.color.cam_xyz), "cam_xyz is 4x3");
    std::memcpy(out->cam_xyz, d.color.cam_xyz, sizeof(out->cam_xyz));

    const auto zero = [](size_t, size_t) { return 0.0f; };
    const auto identity = [](size_t r, size_t c) { return r == c ? 1.0f : 0.0f; };
    for (int i = 0; i < 2; ++i) {
        const auto &src = d.color.dng_color[i];
        out->dng_illuminant[i] = src.illuminant;
        out->dng_has_color_matrix[i] =
            copy_if_present(out->dng_color_matrix[i], src.colormatrix, zero);
        out->dng_has_forward_matrix[i] =
            copy_if_present(out->dng_forward_matrix[i], src.forwardmatrix, zero);
        out->dng_has_calibration[i] =
            copy_if_present(out->dng_calibration[i], src.calibration, identity);
    }

    const auto &levels = d.color.dng_levels;
    out->as_shot_neutral_count = copy_levels(out->as_shot_neutral, levels.asshotneutral);
    out->analog_balance_count = copy_levels(out->analog_balance, levels.analogbalance);
    // LibRaw never sets color.dng_levels.parsedfields (only the per-IFD
    // copies); an absent BaselineExposure keeps its -999 sentinel.
    if (levels.baseline_exposure > -999.0f) {
        out->baseline_exposure = levels.baseline_exposure;
        out->has_baseline_exposure = 1;
    }
    if (levels.LinearResponseLimit > 0.0f) {
        out->linear_response_limit = levels.LinearResponseLimit;
        out->has_linear_response_limit = 1;
    }
}

// A DNG's DefaultCrop, which LibRaw reads (`raw_inset_crops[0]`, raw
// coordinates) but doesn't apply, set as the `cropbox` it does (relative to
// the active area), so a mosaic DNG decodes to the size the DNG SDK gives
// it. Other files' in-camera crops are LibRaw's guesses, so left alone.
void crop_to_dng_default(LibRaw &processor) {
    const libraw_data_t &d = processor.imgdata;
    const libraw_raw_inset_crop_t &crop = d.sizes.raw_inset_crops[0];
    if (!d.idata.dng_version || crop.cleft == 0xffff || crop.ctop == 0xffff || crop.cwidth == 0
        || crop.cheight == 0 || crop.cleft < d.sizes.left_margin
        || crop.ctop < d.sizes.top_margin) {
        return;
    }
    unsigned *box = processor.imgdata.params.cropbox;
    box[0] = crop.cleft - d.sizes.left_margin;
    box[1] = crop.ctop - d.sizes.top_margin;
    box[2] = crop.cwidth;
    box[3] = crop.cheight;
}

// Linear camera RGB: raw color, no white balance, gamma 1.0, no
// auto-brightening, PPG demosaic (faster than AHD), cropped to a DNG's
// DefaultCrop. `adjust_maximum_thr = 0` keeps the camera's white level,
// which LibRaw otherwise lowers to each frame's brightest pixel, giving one
// stack's frames different gains.
void configure_linear(LibRaw &processor) {
    crop_to_dng_default(processor);
    libraw_output_params_t &params = processor.imgdata.params;
    params.output_bps = 16;
    params.output_color = 0;
    params.use_camera_wb = 0;
    params.use_auto_wb = 0;
    params.no_auto_bright = 1;
    params.adjust_maximum_thr = 0.0f;
    params.gamm[0] = 1.0;
    params.gamm[1] = 1.0;
    params.user_qual = 2;
    std::fill_n(params.user_mul, 4, 1.0f);
}

// Whether LibRaw can unpack a preview of this kind into a JPEG or a bitmap.
// JPEG XL and HEVC previews stay compressed in formats we do not decode;
// LibRaw rejects DNG YCbCr previews itself; the rest are rare oddities.
bool usable_preview(LibRaw_internal_thumbnail_formats format) {
    switch (format) {
    case LIBRAW_INTERNAL_THUMBNAIL_JPEG:
    case LIBRAW_INTERNAL_THUMBNAIL_PPM:
    case LIBRAW_INTERNAL_THUMBNAIL_PPM16:
    case LIBRAW_INTERNAL_THUMBNAIL_KODAK_THUMB:
    case LIBRAW_INTERNAL_THUMBNAIL_KODAK_YCBCR:
    case LIBRAW_INTERNAL_THUMBNAIL_KODAK_RGB:
        return true;
    default:
        return false;
    }
}

// The preview list's indices in the order to try them for a thumbnail
// `min_size` on its long edge: big-enough previews smallest first, then
// those of unrecorded size smallest file first, then the too-small ones
// largest first. A preview not shaped like the sensor's `aspect` (stored
// width / height) goes last: Canon's 160x120 THMB letterboxes a 3:2 frame
// in black bars.
std::vector<int> preview_order(const libraw_thumbnail_list_t &list, int min_size, double aspect) {
    struct Entry {
        int index;
        int rank;     // 0 big enough, 1 size unknown, 2 too small, 3 misshapen
        INT64 key;    // ascending within a rank
    };
    std::vector<Entry> entries;
    const int count = std::min(list.thumbcount, LIBRAW_THUMBNAIL_MAXCOUNT);
    for (int i = 0; i < count; ++i) {
        const auto &t = list.thumblist[i];
        if (!usable_preview(t.tformat)) {
            continue;
        }
        const int edge = std::max(t.twidth, t.theight);
        // Previews are stored unrotated, like the sensor dimensions, but a
        // few are stored upright: compare either way round.
        const double shape = t.theight ? double(t.twidth) / t.theight : 0.0;
        const bool misshapen = edge != 0 && aspect > 0.0
            && std::abs(shape / aspect - 1.0) > 0.04 && std::abs(shape * aspect - 1.0) > 0.04;
        if (misshapen) {
            entries.push_back({i, 3, -edge});
        } else if (edge == 0) {
            entries.push_back({i, 1, static_cast<INT64>(t.tlength)});
        } else if (edge >= min_size) {
            entries.push_back({i, 0, edge});
        } else {
            entries.push_back({i, 2, -edge});
        }
    }
    std::stable_sort(entries.begin(), entries.end(), [](const Entry &a, const Entry &b) {
        return a.rank != b.rank ? a.rank < b.rank : a.key < b.key;
    });
    std::vector<int> order;
    for (const auto &e : entries) {
        order.push_back(e.index);
    }
    return order;
}

// The unpacked preview in `p.processor` as JPEG bytes or 8-bit RGB in
// `p.rgb`; false for a kind we don't hand over.
bool take_preview(DtRawPreview &p, int32_t *format, const uint8_t **data, size_t *len) {
    const libraw_thumbnail_t &t = p.processor.imgdata.thumbnail;
    if (!t.thumb || t.tlength == 0) {
        return false;
    }
    if (t.tformat == LIBRAW_THUMBNAIL_JPEG) {
        *format = DT_RAW_PREVIEW_JPEG;
        *data = reinterpret_cast<const uint8_t *>(t.thumb);
        *len = t.tlength;
        return true;
    }
    const size_t pixels = static_cast<size_t>(t.twidth) * t.theight;
    const int colors = t.tcolors;
    const bool wide = t.tformat == LIBRAW_THUMBNAIL_BITMAP16;
    if ((t.tformat != LIBRAW_THUMBNAIL_BITMAP && !wide) || (colors != 1 && colors != 3)
        || pixels == 0 || t.tlength < pixels * colors * (wide ? 2 : 1)) {
        return false;
    }
    p.rgb.resize(pixels * 3);
    for (size_t i = 0; i < pixels; ++i) {
        for (int c = 0; c < 3; ++c) {
            const size_t at = i * colors + (colors == 3 ? c : 0);
            p.rgb[i * 3 + c] = wide
                ? static_cast<uint8_t>(reinterpret_cast<const uint16_t *>(t.thumb)[at] >> 8)
                : static_cast<uint8_t>(t.thumb[at]);
        }
    }
    *format = DT_RAW_PREVIEW_RGB8;
    *data = p.rgb.data();
    *len = p.rgb.size();
    return true;
}

// LibRaw's progress callback: nonzero stops the decode at its next stage
// (LIBRAW_CANCELLED_BY_CALLBACK) once the caller's cancel flag is set.
int stop_when_canceled(void *flag, enum LibRaw_progress, int, int) {
    const auto *cancel = static_cast<const std::atomic<uint8_t> *>(flag);
    return cancel && cancel->load(std::memory_order_relaxed) ? 1 : 0;
}

#ifdef _OPENMP
// OpenMP's team size for this thread's regions while this lives, put back
// after: omp_set_num_threads sticks to the thread, and pool threads decode
// again.
struct TeamSize {
    int before = omp_get_max_threads();
    explicit TeamSize(int threads) {
        if (threads > 0) omp_set_num_threads(threads);
    }
    ~TeamSize() { omp_set_num_threads(before); }
    TeamSize(const TeamSize &) = delete;
    TeamSize &operator=(const TeamSize &) = delete;
};
#endif

// Open, unpack and process `path` in `decode` with the params `configure`
// sets, then let `finish` read what came out. `cancel` (may be NULL) stops
// it between stages; dt_raw_interrupt stops it inside one. `threads` (0:
// LibRaw's own) sizes OpenMP's team for the regions this thread starts.
template <typename Configure, typename Finish>
int32_t decode_with(DtRawDecode *decode, const DtRawPathChar *path, const uint8_t *cancel,
                    int32_t threads, Configure configure, Finish finish) {
    return guarded([&]() -> int32_t {
        LibRaw &processor = decode->processor;
        processor.set_progress_handler(stop_when_canceled, const_cast<uint8_t *>(cancel));
#ifdef _OPENMP
        const TeamSize team(threads);
#else
        (void)threads;
#endif
        int err = processor.open_file(path);
        if (err != LIBRAW_SUCCESS) {
            return err;
        }
        configure(processor);
        if ((err = processor.unpack()) != LIBRAW_SUCCESS
            || (err = processor.dcraw_process()) != LIBRAW_SUCCESS) {
            return err;
        }
        return finish(processor);
    });
}

} // namespace

extern "C" {

int32_t dt_raw_preview(
    const DtRawPathChar *path,
    int32_t min_size,
    DtRawPreview **out,
    int32_t *format,
    int32_t *width,
    int32_t *height,
    int32_t *flip,
    const uint8_t **data,
    size_t *len) {
    if (!path || !out || !format || !width || !height || !flip || !data || !len) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    *out = nullptr;
    std::unique_ptr<DtRawPreview> preview;
    const int32_t err = guarded([&]() -> int32_t {
        preview = std::make_unique<DtRawPreview>();
        LibRaw &processor = preview->processor;
        const int err = processor.open_file(path);
        if (err != LIBRAW_SUCCESS) {
            return err;
        }
        const auto &list = processor.imgdata.thumbs_list;
        const auto &sizes = processor.imgdata.sizes;
        const double aspect = sizes.height ? double(sizes.width) / sizes.height : 0.0;
        for (const int index : preview_order(list, min_size, aspect)) {
            if (processor.unpack_thumb_ex(index) != LIBRAW_SUCCESS
                || !take_preview(*preview, format, data, len)) {
                continue;
            }
            const auto &t = processor.imgdata.thumbnail;
            const auto &item = list.thumblist[index];
            *width = t.twidth;
            *height = t.theight;
            // Kodak loaders rotate as they convert. Others follow the raw's
            // orientation unless they record their own; 0xffff (CR3) and 0
            // (a TIFF preview IFD without the tag, as in a DNG) mean none.
            const bool kodak = item.tformat == LIBRAW_INTERNAL_THUMBNAIL_KODAK_THUMB
                || item.tformat == LIBRAW_INTERNAL_THUMBNAIL_KODAK_YCBCR
                || item.tformat == LIBRAW_INTERNAL_THUMBNAIL_KODAK_RGB;
            const bool own = item.tflip != 0 && item.tflip != 0xffff;
            *flip = kodak ? 0 : own ? item.tflip : processor.imgdata.sizes.flip;
            return LIBRAW_SUCCESS;
        }
        return LIBRAW_NO_THUMBNAIL;
    });
    if (err != LIBRAW_SUCCESS) {
        return err;
    }
    *out = preview.release();
    return LIBRAW_SUCCESS;
}

void dt_raw_preview_free(DtRawPreview *preview) { delete preview; }

DtRawDecode *dt_raw_decoder(void) { return new (std::nothrow) DtRawDecode(); }

void dt_raw_interrupt(DtRawDecode *decode) {
    if (decode) decode->processor.setCancelFlag();
}

int32_t dt_raw_decode_half(
    const DtRawPathChar *path,
    DtRawDecode *decode,
    int32_t *width,
    int32_t *height,
    const uint8_t *cancel,
    int32_t threads) {
    if (!path || !decode || !width || !height) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return decode_with(
        decode, path, cancel, threads,
        [](LibRaw &processor) {
            libraw_output_params_t &params = processor.imgdata.params;
            params.half_size = 1;
            params.use_camera_wb = 1;
            params.output_bps = 8;
        },
        [&](LibRaw &processor) -> int32_t {
            int channels = 0, bps = 0;
            processor.get_mem_image_format(width, height, &channels, &bps);
            return channels == 3 && bps == 8 ? LIBRAW_SUCCESS : LIBRAW_UNSPECIFIED_ERROR;
        });
}

int32_t dt_raw_copy_pixels8(DtRawDecode *decode, uint8_t *dest, size_t count) {
    if (!decode || !dest) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return guarded([&]() -> int32_t {
        int width = 0, height = 0, channels = 0, bps = 0;
        decode->processor.get_mem_image_format(&width, &height, &channels, &bps);
        if (bps != 8 || count != static_cast<size_t>(width) * height * channels) {
            return LIBRAW_UNSPECIFIED_ERROR;
        }
        return decode->processor.copy_mem_image(dest, width * channels, 0);
    });
}

int32_t dt_raw_read_metadata(const DtRawPathChar *path, DtRawMetadata *out) {
    if (!path || !out) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return guarded([&]() -> int32_t {
        const auto processor = std::make_unique<LibRaw>();
        const int err = processor->open_file(path);
        if (err == LIBRAW_SUCCESS) {
            fill_metadata(out, processor->imgdata);
        }
        return err;
    });
}

int32_t dt_raw_read_size(const DtRawPathChar *path, int32_t *width, int32_t *height) {
    if (!path || !width || !height) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return guarded([&]() -> int32_t {
        const auto processor = std::make_unique<LibRaw>();
        const int err = processor->open_file(path);
        if (err != LIBRAW_SUCCESS) {
            return err;
        }
        // Before processing, get_mem_image_format already gives the size
        // dcraw_process will (Fuji's rotation, pixel aspect, a turned
        // frame's swapped axes) for the decode's params.
        configure_linear(*processor);
        int channels = 0, bps = 0;
        processor->get_mem_image_format(width, height, &channels, &bps);
        // A DNG's crop is applied as it unpacks: its size now, upright.
        const unsigned *box = processor->imgdata.params.cropbox;
        if (~box[2] && ~box[3]) {
            const bool turned = processor->imgdata.sizes.flip & 4;
            *width = static_cast<int>(turned ? box[3] : box[2]);
            *height = static_cast<int>(turned ? box[2] : box[3]);
        }
        return *width > 0 && *height > 0 ? LIBRAW_SUCCESS : LIBRAW_UNSPECIFIED_ERROR;
    });
}

int32_t dt_raw_decode(
    const DtRawPathChar *path,
    DtRawMetadata *meta,
    DtRawDecode *decode,
    int32_t *width,
    int32_t *height,
    int32_t *channels,
    const uint8_t *cancel,
    int32_t threads) {
    if (!path || !meta || !decode || !width || !height || !channels) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return decode_with(decode, path, cancel, threads, configure_linear,
                       [&](LibRaw &processor) -> int32_t {
        fill_metadata(meta, processor.imgdata);
        int bps = 0;
        processor.get_mem_image_format(width, height, channels, &bps);
        return bps == 16 ? LIBRAW_SUCCESS : LIBRAW_UNSPECIFIED_ERROR;
    });
}

int32_t dt_raw_copy_pixels(DtRawDecode *decode, uint16_t *dest, size_t count) {
    if (!decode || !dest) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
    return guarded([&]() -> int32_t {
        int width = 0, height = 0, channels = 0, bps = 0;
        decode->processor.get_mem_image_format(&width, &height, &channels, &bps);
        // `dest` is 16-bit samples: anything else would be copied past it.
        if (bps != 16 || width <= 0 || height <= 0 || channels <= 0) {
            return LIBRAW_UNSPECIFIED_ERROR;
        }
        if (count != static_cast<size_t>(width) * height * channels) {
            return LIBRAW_UNSPECIFIED_ERROR;
        }
        const int stride = width * channels * static_cast<int>(sizeof(uint16_t));
        return decode->processor.copy_mem_image(dest, stride, 0);
    });
}

void dt_raw_decode_free(DtRawDecode *decode) { delete decode; }

const char *dt_raw_strerror(int32_t code) { return libraw_strerror(code); }

void dt_raw_camera_name(const char *make, const char *model, const char *unique, char *out,
                       size_t capacity) {
    if (!out || capacity == 0) return;
    copy_text(camera_name(make ? make : "", model ? model : "", unique ? unique : ""), out,
              capacity);
}

int32_t dt_raw_parallel_threads(void) {
    int32_t threads = 0;
#pragma omp parallel
    {
#pragma omp atomic
        threads++;
    }
    return threads;
}

} // extern "C"
