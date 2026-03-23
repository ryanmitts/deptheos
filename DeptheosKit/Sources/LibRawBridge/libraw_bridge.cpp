#include "libraw_bridge.h"
#include "libraw/libraw.h"
#include <cstring>
#include <cmath>
#include <pthread.h>

// LibRaw's dcraw_process() uses large stack allocations (demosaicing buffers).
// Dispatch queue worker threads have 512KB stacks which can overflow.
// Run processing on a thread with 8MB stack to match the main thread.
static const size_t LIBRAW_STACK_SIZE = 8 * 1024 * 1024;

struct ThreadArgs {
    bool (*func)(void *);
    void *ctx;
    bool result;
};

static void *thread_trampoline(void *arg) {
    auto *args = static_cast<ThreadArgs *>(arg);
    args->result = args->func(args->ctx);
    return nullptr;
}

static bool run_with_large_stack(bool (*func)(void *), void *ctx) {
    pthread_t thread;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, LIBRAW_STACK_SIZE);

    ThreadArgs args = { func, ctx, false };
    int err = pthread_create(&thread, &attr, thread_trampoline, &args);
    pthread_attr_destroy(&attr);
    if (err != 0) {
        return func(ctx);
    }
    pthread_join(thread, nullptr);
    return args.result;
}

static std::string safe_string(const char *src) {
    return src ? std::string(src) : std::string();
}

static std::vector<float> floats_from_array(const float *src, size_t count) {
    return std::vector<float>(src, src + count);
}

static void fill_metadata(RawReadResult &result, libraw_data_t &d) {
    result.make = safe_string(d.idata.make);
    result.model = safe_string(d.idata.model);
    result.software = safe_string(d.idata.software);
    result.uniqueCameraModel = safe_string(d.color.UniqueCameraModel);
    result.artist = safe_string(d.other.artist);
    result.description = safe_string(d.other.desc);

    switch (d.sizes.flip) {
        case 0:  result.orientation = 1; break;
        case 3:  result.orientation = 3; break;
        case 5:  result.orientation = 8; break;
        case 6:  result.orientation = 6; break;
        default: result.orientation = 1; break;
    }

    result.isoSpeed  = d.other.iso_speed;
    result.shutter   = d.other.shutter;
    result.aperture  = d.other.aperture;
    result.focalLen  = d.other.focal_len;
    result.timestamp = static_cast<int32_t>(d.other.timestamp);

    result.camMul = floats_from_array(d.color.cam_mul, 4);
    result.preMul = floats_from_array(d.color.pre_mul, 4);

    {
        std::vector<float> v(12);
        for (int r = 0; r < 4; r++)
            for (int c = 0; c < 3; c++)
                v[r * 3 + c] = d.color.cam_xyz[r][c];
        result.camXYZ = std::move(v);
    }
    {
        std::vector<float> v(12);
        for (int r = 0; r < 3; r++)
            for (int c = 0; c < 4; c++)
                v[r * 4 + c] = d.color.rgb_cam[r][c];
        result.rgbCam = std::move(v);
    }

    result.dngColor.resize(2);
    for (int i = 0; i < 2; i++) {
        auto &src = d.color.dng_color[i];
        auto &dst = result.dngColor[i];
        dst.illuminant = src.illuminant;

        dst.colorMatrix.resize(12);
        dst.hasColorMatrix = false;
        for (int r = 0; r < 4; r++)
            for (int c = 0; c < 3; c++) {
                dst.colorMatrix[r * 3 + c] = src.colormatrix[r][c];
                if (src.colormatrix[r][c] != 0.0f) dst.hasColorMatrix = true;
            }
        if (!dst.hasColorMatrix) dst.colorMatrix.clear();

        dst.forwardMatrix.resize(12);
        dst.hasForwardMatrix = false;
        for (int r = 0; r < 3; r++)
            for (int c = 0; c < 4; c++) {
                dst.forwardMatrix[r * 4 + c] = src.forwardmatrix[r][c];
                if (src.forwardmatrix[r][c] != 0.0f) dst.hasForwardMatrix = true;
            }
        if (!dst.hasForwardMatrix) dst.forwardMatrix.clear();

        dst.calibration.resize(16);
        dst.hasCalibration = false;
        for (int r = 0; r < 4; r++)
            for (int c = 0; c < 4; c++) {
                dst.calibration[r * 4 + c] = src.calibration[r][c];
                if (r != c && src.calibration[r][c] != 0.0f) dst.hasCalibration = true;
                if (r == c && src.calibration[r][c] != 1.0f) dst.hasCalibration = true;
            }
        if (!dst.hasCalibration) dst.calibration.clear();
    }

    result.cblack = { d.color.cblack[0], d.color.cblack[1],
                      d.color.cblack[2], d.color.cblack[3] };
    result.black   = d.color.black;
    result.maximum = d.color.maximum;
    result.linearMax = { d.color.linear_max[0], d.color.linear_max[1],
                         d.color.linear_max[2], d.color.linear_max[3] };
    result.rawColors = d.idata.colors;

    auto &dl = d.color.dng_levels;
    for (int i = 0; i < 4; i++) {
        if (dl.asshotneutral[i] != 0.0f)
            result.asShotNeutral.push_back(dl.asshotneutral[i]);
        else
            break;
    }
    for (int i = 0; i < 4; i++) {
        if (dl.analogbalance[i] != 0.0f)
            result.analogBalance.push_back(dl.analogbalance[i]);
        else
            break;
    }

    if ((dl.parsedfields & (1 << 3)) != 0)
        result.baselineExposure = dl.baseline_exposure;
    if (dl.LinearResponseLimit > 0.0f)
        result.linearResponseLimit = dl.LinearResponseLimit;

    auto &ls = d.lens;
    result.lens.make = safe_string(ls.LensMake);
    result.lens.model = safe_string(ls.Lens);
    result.lens.serial = safe_string(ls.LensSerial);
    result.lens.minFocal = ls.MinFocal;
    result.lens.maxFocal = ls.MaxFocal;
    result.lens.maxApAtMinFocal = ls.MaxAp4MinFocal;
    result.lens.maxApAtMaxFocal = ls.MaxAp4MaxFocal;
    result.lens.focalLengthIn35mm = ls.FocalLengthIn35mmFormat;

    auto &gps = d.other.parsed_gps;
    result.gps.parsed = (gps.gpsparsed != 0);
    if (result.gps.parsed) {
        result.gps.latitude = floats_from_array(gps.latitude, 3);
        result.gps.longitude = floats_from_array(gps.longitude, 3);
        result.gps.altitude = gps.altitude;
        result.gps.latref   = gps.latref;
        result.gps.longref  = gps.longref;
        result.gps.altref   = gps.altref;
    }
}

static bool extract_image(LibRaw &processor, RawReadResult &result) {
    int errc = 0;
    libraw_processed_image_t *img = processor.dcraw_make_mem_image(&errc);
    if (!img || errc != LIBRAW_SUCCESS) return false;

    int w = img->width;
    int h = img->height;
    int ch = img->colors;
    size_t pixel_count = static_cast<size_t>(w) * h * ch;

    result.pixels.resize(pixel_count);
    memcpy(result.pixels.data(), img->data, pixel_count * sizeof(uint16_t));
    LibRaw::dcraw_clear_mem(img);

    result.width    = static_cast<int32_t>(w);
    result.height   = static_cast<int32_t>(h);
    result.channels = static_cast<int32_t>(ch);
    result.bits     = 16;
    return true;
}

static bool process_raw_srgb(LibRaw &processor, RawReadResult &result) {
    processor.imgdata.params.output_bps = 16;
    processor.imgdata.params.output_color = 1;  // sRGB
    processor.imgdata.params.use_camera_wb = 1;
    processor.imgdata.params.no_auto_bright = 1;

    int err = processor.unpack();
    if (err != LIBRAW_SUCCESS) return false;

    err = processor.dcraw_process();
    if (err != LIBRAW_SUCCESS) return false;

    return extract_image(processor, result);
}

static bool process_raw_linear(LibRaw &processor, RawReadResult &result) {
    processor.imgdata.params.output_bps = 16;
    processor.imgdata.params.output_color = 0;  // raw color
    processor.imgdata.params.use_camera_wb = 0;
    processor.imgdata.params.use_auto_wb = 0;
    processor.imgdata.params.no_auto_bright = 1;
    processor.imgdata.params.gamm[0] = 1.0;  // linear
    processor.imgdata.params.gamm[1] = 1.0;
    processor.imgdata.params.user_qual = 2;  // PPG demosaic (faster than AHD default)

    processor.imgdata.params.user_mul[0] = 1.0f;
    processor.imgdata.params.user_mul[1] = 1.0f;
    processor.imgdata.params.user_mul[2] = 1.0f;
    processor.imgdata.params.user_mul[3] = 1.0f;

    int err = processor.unpack();
    if (err != LIBRAW_SUCCESS) return false;

    err = processor.dcraw_process();
    if (err != LIBRAW_SUCCESS) return false;

    return extract_image(processor, result);
}

struct ReadRawCtx {
    const char *path;
    const void *data;
    size_t size;
    RawReadResult *result;
    bool linear;
};

static bool read_raw_file_impl(void *arg) {
    auto *ctx = static_cast<ReadRawCtx *>(arg);
    LibRaw processor;
    int err = processor.open_file(ctx->path);
    if (err != LIBRAW_SUCCESS) return false;

    bool ok = ctx->linear
        ? process_raw_linear(processor, *ctx->result)
        : process_raw_srgb(processor, *ctx->result);
    if (!ok) return false;
    fill_metadata(*ctx->result, processor.imgdata);
    return true;
}

static bool read_raw_bytes_impl(void *arg) {
    auto *ctx = static_cast<ReadRawCtx *>(arg);
    LibRaw processor;
    int err = processor.open_buffer(ctx->data, ctx->size);
    if (err != LIBRAW_SUCCESS) return false;

    bool ok = ctx->linear
        ? process_raw_linear(processor, *ctx->result)
        : process_raw_srgb(processor, *ctx->result);
    if (!ok) return false;
    fill_metadata(*ctx->result, processor.imgdata);
    return true;
}

std::optional<RawReadResult> deptheos_read_raw(const std::string &path) {
    RawReadResult result;
    ReadRawCtx ctx = { path.c_str(), nullptr, 0, &result, false };
    if (!run_with_large_stack(read_raw_file_impl, &ctx)) return std::nullopt;
    return result;
}

std::optional<RawReadResult> deptheos_read_raw_from_bytes(const uint8_t *bytes, size_t byte_count) {
    RawReadResult result;
    ReadRawCtx ctx = { nullptr, bytes, byte_count, &result, false };
    if (!run_with_large_stack(read_raw_bytes_impl, &ctx)) return std::nullopt;
    return result;
}

std::optional<RawReadResult> deptheos_read_raw_linear(const std::string &path) {
    RawReadResult result;
    ReadRawCtx ctx = { path.c_str(), nullptr, 0, &result, true };
    if (!run_with_large_stack(read_raw_file_impl, &ctx)) return std::nullopt;
    return result;
}

std::optional<RawReadResult> deptheos_read_raw_linear_from_bytes(const uint8_t *bytes, size_t byte_count) {
    RawReadResult result;
    ReadRawCtx ctx = { nullptr, bytes, byte_count, &result, true };
    if (!run_with_large_stack(read_raw_bytes_impl, &ctx)) return std::nullopt;
    return result;
}

// --- Two-phase API ---

struct OpenRawCtx {
    const char *path;
    const void *data;
    size_t size;
    RawDecodeHandle *handle;
};

static bool open_raw_file_impl(void *arg) {
    auto *ctx = static_cast<OpenRawCtx *>(arg);
    LibRaw processor;
    int err = ctx->path
        ? processor.open_file(ctx->path)
        : processor.open_buffer(ctx->data, ctx->size);
    if (err != LIBRAW_SUCCESS) return false;

    // Configure for linear output
    processor.imgdata.params.output_bps = 16;
    processor.imgdata.params.output_color = 0;
    processor.imgdata.params.use_camera_wb = 0;
    processor.imgdata.params.use_auto_wb = 0;
    processor.imgdata.params.no_auto_bright = 1;
    processor.imgdata.params.gamm[0] = 1.0;
    processor.imgdata.params.gamm[1] = 1.0;
    processor.imgdata.params.user_qual = 2;  // PPG demosaic (faster than AHD default)
    processor.imgdata.params.user_mul[0] = 1.0f;
    processor.imgdata.params.user_mul[1] = 1.0f;
    processor.imgdata.params.user_mul[2] = 1.0f;
    processor.imgdata.params.user_mul[3] = 1.0f;

    err = processor.unpack();
    if (err != LIBRAW_SUCCESS) return false;

    err = processor.dcraw_process();
    if (err != LIBRAW_SUCCESS) return false;

    // Extract image — keep pointer alive (no memcpy into vector)
    int errc = 0;
    libraw_processed_image_t *img = processor.dcraw_make_mem_image(&errc);
    if (!img || errc != LIBRAW_SUCCESS) return false;

    ctx->handle->_internal = img;
    ctx->handle->metadata.width = static_cast<int32_t>(img->width);
    ctx->handle->metadata.height = static_cast<int32_t>(img->height);
    ctx->handle->metadata.channels = static_cast<int32_t>(img->colors);
    ctx->handle->metadata.bits = 16;

    fill_metadata(ctx->handle->metadata, processor.imgdata);
    return true;
}

std::optional<RawDecodeHandle> deptheos_open_raw_linear(const std::string &path) {
    RawDecodeHandle handle;
    OpenRawCtx ctx = { path.c_str(), nullptr, 0, &handle };
    if (!run_with_large_stack(open_raw_file_impl, &ctx)) return std::nullopt;
    return handle;
}

std::optional<RawDecodeHandle> deptheos_open_raw_linear_from_bytes(const uint8_t *bytes, size_t byte_count) {
    RawDecodeHandle handle;
    OpenRawCtx ctx = { nullptr, bytes, byte_count, &handle };
    if (!run_with_large_stack(open_raw_file_impl, &ctx)) return std::nullopt;
    return handle;
}

void deptheos_copy_raw_pixels(RawDecodeHandle &handle, void *dest) {
    auto *img = static_cast<libraw_processed_image_t *>(handle._internal);
    if (!img) return;
    size_t bytes = static_cast<size_t>(img->width) * img->height * img->colors * sizeof(uint16_t);
    memcpy(dest, img->data, bytes);
}

void *deptheos_raw_pixel_data(RawDecodeHandle &handle) {
    auto *img = static_cast<libraw_processed_image_t *>(handle._internal);
    return img ? img->data : nullptr;
}

void deptheos_close_raw(RawDecodeHandle &handle) {
    if (handle._internal) {
        LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t *>(handle._internal));
        handle._internal = nullptr;
    }
}
