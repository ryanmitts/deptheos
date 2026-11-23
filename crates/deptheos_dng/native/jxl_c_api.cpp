#include "jxl_c_api.h"
#include "guard.h"

#include <jxl/decode.h>
#include <jxl/encode.h>
#include <jxl/thread_parallel_runner.h>

#include <algorithm>
#include <atomic>
#include <memory>
#include <vector>

namespace {

// A thread pool for one encode or decode, declared before the encoder or
// decoder so that is destroyed first. Don't keep it between calls: a
// `thread_local` pool is destroyed at thread exit, which on Windows runs
// under the loader lock its exiting workers wait for, deadlocking every
// thread start and exit. Making one takes about a millisecond.
//
// `threads` (0: one per core) is from the CPU budget (ADR-143). Once
// `cancel` (may be NULL) is set the runner refuses work, which libjxl turns
// into JXL_DEC_ERROR (or JXL_ENC_ERROR) within a stage. That mapping is
// libjxl's code, not its documented API, so a test pins it (jxl.rs).
struct Runner {
    Runner(int32_t threads, const uint8_t *cancel)
        : pool(JxlThreadParallelRunnerCreate(
              nullptr, threads > 0 ? static_cast<size_t>(threads)
                                   : JxlThreadParallelRunnerDefaultNumWorkerThreads())),
          cancel(reinterpret_cast<const std::atomic<uint8_t> *>(cancel)) {}
    ~Runner() { JxlThreadParallelRunnerDestroy(pool); }
    Runner(const Runner &) = delete;
    Runner &operator=(const Runner &) = delete;

    void *pool;
    const std::atomic<uint8_t> *cancel;

    bool canceled() const { return cancel && cancel->load(std::memory_order_relaxed); }

    // A JxlParallelRunner over `pool` that refuses to run once canceled.
    static JxlParallelRetCode Run(void *runner, void *jpegxl_opaque, JxlParallelRunInit init,
                                  JxlParallelRunFunction func, uint32_t start_range,
                                  uint32_t end_range) {
        const Runner &self = *static_cast<const Runner *>(runner);
        if (self.canceled()) {
            return JXL_PARALLEL_RET_RUNNER_ERROR;
        }
        return JxlThreadParallelRunner(self.pool, jpegxl_opaque, init, func, start_range,
                                       end_range);
    }
};

struct Encoder {
    JxlEncoder *enc = JxlEncoderCreate(nullptr);
    ~Encoder() { JxlEncoderDestroy(enc); }
};

struct Decoder {
    JxlDecoder *dec = JxlDecoderCreate(nullptr);
    ~Decoder() { JxlDecoderDestroy(dec); }
};

// RGB u16 or gray u16; false for any other.
bool pixel_format(int32_t channels, int32_t bits, JxlPixelFormat *format) {
    if (channels == 3 && bits == 16) {
        *format = {3, JXL_TYPE_UINT16, JXL_NATIVE_ENDIAN, 0};
    } else if (channels == 1 && bits == 16) {
        *format = {1, JXL_TYPE_UINT16, JXL_NATIVE_ENDIAN, 0};
    } else {
        return false;
    }
    return true;
}

// The caller's samples, handed to libjxl a rectangle at a time
// (JxlEncoderAddChunkedFrame), so it never copies the whole picture.
struct Chunks {
    const uint8_t *samples;
    size_t stride;
    size_t pixel_bytes;
    JxlPixelFormat format;

    static void Format(void *opaque, JxlPixelFormat *format) {
        *format = static_cast<Chunks *>(opaque)->format;
    }
    static const void *At(void *opaque, size_t x, size_t y, size_t, size_t, size_t *row_offset) {
        const Chunks &self = *static_cast<Chunks *>(opaque);
        *row_offset = self.stride;
        return self.samples + y * self.stride + x * self.pixel_bytes;
    }
    static void NoExtraFormat(void *, size_t, JxlPixelFormat *) {}
    static const void *NoExtra(void *, size_t, size_t, size_t, size_t, size_t, size_t *) {
        return nullptr;
    }
    static void Release(void *, const void *) {}
};

// The codestream, handed to the caller's writer a part at a time
// (JxlEncoderSetOutputProcessor), with a seek back to fill in the table of
// contents: no more than one buffer of the file is held.
struct Output {
    void *ctx;
    DtJxlWrite write;
    DtJxlSeek seek;
    std::vector<uint8_t> buffer;
    bool failed = false;

    static void *Get(void *opaque, size_t *size) {
        Output &self = *static_cast<Output *>(opaque);
        if (self.failed) {
            *size = 0;
            return nullptr;
        }
        self.buffer.resize(std::max<size_t>(*size, 1 << 20));
        *size = self.buffer.size();
        return self.buffer.data();
    }
    static void Release(void *opaque, size_t written) {
        Output &self = *static_cast<Output *>(opaque);
        if (!self.failed && written > 0 && !self.write(self.ctx, self.buffer.data(), written)) {
            self.failed = true;
        }
    }
    static void Seek(void *opaque, uint64_t position) {
        Output &self = *static_cast<Output *>(opaque);
        if (!self.failed && !self.seek(self.ctx, position)) {
            self.failed = true;
        }
    }
    static void Finalized(void *, uint64_t) {}
};

int32_t encode_impl(const void *samples, size_t sample_count, uint32_t width, uint32_t height,
                    int32_t channels, int32_t bits, int32_t effort, int32_t linear,
                    int32_t threads, const uint8_t *cancel, void *ctx, DtJxlWrite write,
                    DtJxlSeek seek) {
    JxlPixelFormat format;
    if (!samples || !write || !seek || width == 0 || height == 0 ||
        !pixel_format(channels, bits, &format) ||
        sample_count != static_cast<size_t>(width) * height * channels) {
        return 0;
    }
    Runner runner(threads, cancel);
    Encoder encoder;
    JxlEncoder *enc = encoder.enc;
    if (!runner.pool || !enc ||
        JxlEncoderSetParallelRunner(enc, Runner::Run, &runner) != JXL_ENC_SUCCESS) {
        return 0;
    }

    JxlBasicInfo info;
    JxlEncoderInitBasicInfo(&info);
    info.xsize = width;
    info.ysize = height;
    info.bits_per_sample = bits;
    info.num_color_channels = channels;
    // Lossless needs the original color space.
    info.uses_original_profile = JXL_TRUE;
    if (JxlEncoderSetBasicInfo(enc, &info) != JXL_ENC_SUCCESS) {
        return 0;
    }
    JxlColorEncoding color;
    const JXL_BOOL gray = channels == 1 ? JXL_TRUE : JXL_FALSE;
    if (linear) {
        JxlColorEncodingSetToLinearSRGB(&color, gray);
    } else {
        JxlColorEncodingSetToSRGB(&color, gray);
    }
    if (JxlEncoderSetColorEncoding(enc, &color) != JXL_ENC_SUCCESS) {
        return 0;
    }

    Output output{ctx, write, seek, {}};
    JxlEncoderOutputProcessor processor{&output, Output::Get, Output::Release, Output::Seek,
                                        Output::Finalized};
    const size_t pixel_bytes = static_cast<size_t>(channels) * (bits / 8);
    Chunks chunks{static_cast<const uint8_t *>(samples), pixel_bytes * width, pixel_bytes,
                  format};
    JxlChunkedFrameInputSource source{&chunks,         Chunks::Format,  Chunks::At,
                                      Chunks::NoExtraFormat, Chunks::NoExtra, Chunks::Release};
    JxlEncoderFrameSettings *frame = JxlEncoderFrameSettingsCreate(enc, nullptr);
    // With an output processor set, a chunked frame is encoded and written
    // a group at a time as it is added.
    if (!frame || JxlEncoderSetFrameLossless(frame, JXL_TRUE) != JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(frame, JXL_ENC_FRAME_SETTING_EFFORT, effort) !=
            JXL_ENC_SUCCESS ||
        JxlEncoderSetOutputProcessor(enc, processor) != JXL_ENC_SUCCESS ||
        JxlEncoderAddChunkedFrame(frame, JXL_TRUE, source) != JXL_ENC_SUCCESS) {
        return 0;
    }
    return output.failed ? 0 : 1;
}

int32_t info_impl(const uint8_t *data, size_t size, uint32_t *width, uint32_t *height,
                  uint32_t *channels) {
    // Checked first: libjxl logs a malformed signature to stderr.
    JxlSignature signature = JxlSignatureCheck(data, size);
    if (signature != JXL_SIG_CODESTREAM && signature != JXL_SIG_CONTAINER) {
        ThrowBadFormat();
    }
    Decoder decoder;
    JxlDecoder *dec = decoder.dec;
    if (!dec || JxlDecoderSubscribeEvents(dec, JXL_DEC_BASIC_INFO) != JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(dec, data, size) != JXL_DEC_SUCCESS) {
        return 0;
    }
    JxlDecoderCloseInput(dec);
    if (JxlDecoderProcessInput(dec) != JXL_DEC_BASIC_INFO) {
        ThrowBadFormat();
    }
    JxlBasicInfo info;
    if (JxlDecoderGetBasicInfo(dec, &info) != JXL_DEC_SUCCESS) {
        return 0;
    }
    *width = info.xsize;
    *height = info.ysize;
    *channels = info.num_color_channels + info.num_extra_channels;
    return 1;
}

int32_t decode_impl(const uint8_t *data, size_t size, void *out, size_t sample_count,
                    int32_t channels, int32_t bits, int32_t threads, const uint8_t *cancel) {
    JxlPixelFormat format;
    if (!out || !pixel_format(channels, bits, &format)) {
        return 0;
    }
    Runner runner(threads, cancel);
    Decoder decoder;
    JxlDecoder *dec = decoder.dec;
    if (!runner.pool || !dec ||
        JxlDecoderSetParallelRunner(dec, Runner::Run, &runner) !=
            JXL_DEC_SUCCESS ||
        JxlDecoderSubscribeEvents(dec, JXL_DEC_BASIC_INFO | JXL_DEC_FULL_IMAGE) !=
            JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(dec, data, size) != JXL_DEC_SUCCESS) {
        return 0;
    }
    JxlDecoderCloseInput(dec);
    bool decoded = false;
    for (;;) {
        switch (JxlDecoderProcessInput(dec)) {
        case JXL_DEC_BASIC_INFO: {
            JxlBasicInfo info;
            if (JxlDecoderGetBasicInfo(dec, &info) != JXL_DEC_SUCCESS ||
                info.num_color_channels != static_cast<uint32_t>(channels) ||
                static_cast<size_t>(info.xsize) * info.ysize * channels != sample_count) {
                return 0;
            }
            break;
        }
        case JXL_DEC_NEED_IMAGE_OUT_BUFFER:
            if (JxlDecoderSetImageOutBuffer(dec, &format, out, sample_count * (bits / 8)) !=
                JXL_DEC_SUCCESS) {
                return 0;
            }
            break;
        case JXL_DEC_FULL_IMAGE:
            decoded = true;
            break;
        case JXL_DEC_SUCCESS:
            return decoded ? 1 : 0;
        default:
            // libjxl's error, which a canceled runner also gives.
            if (runner.canceled()) ThrowUserCanceled();
            ThrowBadFormat();
        }
    }
}

} // namespace

// libjxl is C++ underneath and can throw (std::bad_alloc), so every entry
// point is guarded (guard.h). What the decoder rejects is thrown as
// dng_error_bad_format.

extern "C" int32_t dt_jxl_encode(const void *samples, size_t sample_count, uint32_t width,
                                 uint32_t height, int32_t channels, int32_t bits, int32_t effort,
                                 int32_t linear, int32_t threads, const uint8_t *cancel,
                                 void *ctx, DtJxlWrite write, DtJxlSeek seek,
                                 DtStatus *status) {
    return guarded<int32_t>("jxl_encode", status, [&] {
        return encode_impl(samples, sample_count, width, height, channels, bits, effort, linear,
                           threads, cancel, ctx, write, seek);
    });
}

extern "C" int32_t dt_jxl_info(const uint8_t *data, size_t size, uint32_t *width,
                               uint32_t *height, uint32_t *channels, DtStatus *status) {
    if (!data || !width || !height || !channels) {
        return 0;
    }
    return guarded<int32_t>("jxl_info", status,
                            [&] { return info_impl(data, size, width, height, channels); });
}

extern "C" int32_t dt_jxl_decode(const uint8_t *data, size_t size, void *out,
                                 size_t sample_count, int32_t channels, int32_t bits,
                                 int32_t threads, const uint8_t *cancel, DtStatus *status) {
    return guarded<int32_t>("jxl_decode", status, [&] {
        return decode_impl(data, size, out, sample_count, channels, bits, threads, cancel);
    });
}
