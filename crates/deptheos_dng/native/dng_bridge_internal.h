// DngBridge internals shared by its .cpp files: SDK includes, the
// multithreaded host and the metadata helpers.
#pragma once

#include "dng_bridge.h"
#include "guard.h"

#include <stdbool.h>
#include <stdexcept>
#include <cstring>
#include <memory>
#include <cstdio>
#include <cstdarg>
#include <cmath>
#include <algorithm>
#include <atomic>
#include <exception>
#include <mutex>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

#include "dng_abort_sniffer.h"
#include "dng_host.h"
#include "dng_camera_profile.h"
#include "dng_file_stream.h"
#include "dng_memory_stream.h"
#include "dng_info.h"
#include "dng_negative.h"
#include "dng_image.h"
#include "dng_pixel_buffer.h"
#include "dng_simple_image.h"
#include "dng_image_writer.h"
#include "dng_render.h"
#include "dng_preview.h"
#include "dng_color_space.h"
#include "dng_color_spec.h"
#include "dng_exceptions.h"
#include "dng_tag_codes.h"
#include "dng_tag_values.h"
#include "dng_sdk_limits.h"
#include "dng_area_task.h"
#include "dng_jpeg_image.h"

// A thread with the engine's 8 MiB stack (ADR-143), which std::thread can't
// set (Apple's default is 512 KB). `run(arg)` must not throw. NULL if it
// could not start.
struct engine_thread;
engine_thread *start_engine_thread(void (*run)(void *), void *arg);
// Joins and frees the thread.
void join_engine_thread(engine_thread *thread);

// A negative whose black-level step can be left to the copy out
// (dng_fast_read.cpp). With `fDeferLinearization` set before
// BuildStage2Image, one that needs only per-plane lookup tables keeps stage 1
// as stage 2 and saves the tables: the SDK's pass is a full-frame memory
// round trip per plane, and the copy out reads the frame anyway.
class deptheos_negative : public dng_negative {
public:
    static dng_negative *Make(dng_host &host);

    bool fDeferLinearization = false;

    // Whether stage 2 is stage 1 still waiting for the tables.
    bool LinearizationDeferred() const { return fDeferred; }
    // Where stage 2 begins in the (stage 1) image.
    const dng_point &DeferredOrigin() const { return fDeferredOrigin; }
    const dng_rect &DeferredArea() const { return fDeferredArea; }
    const uint16 *DeferredTable(uint32 plane) const { return fTables[plane].data(); }

protected:
    explicit deptheos_negative(dng_host &host) : dng_negative(host) {}
    void DoBuildStage2(dng_host &host) override;

private:
    bool fDeferred = false;
    dng_point fDeferredOrigin;
    dng_rect fDeferredArea;
    std::vector<uint16> fTables[kMaxColorPlanes];
};

// Throws dng_error_user_canceled once the caller's cancel flag is set
// (ADR-143). Thread safe, so area tasks sniff from every thread.
class deptheos_sniffer : public dng_abort_sniffer {
public:
    explicit deptheos_sniffer(const uint8_t *cancel)
        : fCancel(reinterpret_cast<const std::atomic<uint8_t> *>(cancel)) {}

    bool ThreadSafe() const override { return true; }

    bool Canceled() const { return fCancel && fCancel->load(std::memory_order_relaxed); }

protected:
    void Sniff() override {
        if (Canceled()) ThrowUserCanceled();
    }

private:
    const std::atomic<uint8_t> *fCancel;
};

// The host every read, render and write runs on. Its SDK sizes stay 0, so
// images are read at full size, never a preview's. `threads` (0: every core,
// up to the SDK's limit) caps its threads; `cancel` may be NULL.
class deptheos_host : public dng_host {
public:
    explicit deptheos_host(uint32 threads = 0, const uint8_t *cancel = nullptr)
        : dng_host()
        , fThreads(threads ? threads
                           : std::min((uint32)std::thread::hardware_concurrency(),
                                      (uint32)kMaxMPThreads))
        , fSniffer(cancel)
    {
        if (fThreads < 1) fThreads = 1;
        if (fThreads > (uint32)kMaxMPThreads) fThreads = (uint32)kMaxMPThreads;
        SetSniffer(&fSniffer);
        deptheos_dng_initialize();
        dng_log(DNG_LOG_DEBUG, "[deptheos_host] Created with %u threads", fThreads);
    }

    // For the bridge's own loops, which bypass the SDK's tasks.
    void CheckCanceled() { dng_abort_sniffer::SniffForAbort(&fSniffer); }

    dng_negative *Make_dng_negative() override { return deptheos_negative::Make(*this); }

    uint32 PerformAreaTaskThreads() override {
        return fThreads;
    }

    void PerformAreaTask(dng_area_task &task,
                         const dng_rect &area,
                         dng_area_task_progress *progress = nullptr) override {

        uint32 threadCount = std::min(fThreads, task.MaxThreads());

        dng_point tileSize(task.FindTileSize(area));
        uint32 tileRows = (area.H() + tileSize.v - 1) / tileSize.v;
        uint32 tileCols = (area.W() + tileSize.h - 1) / tileSize.h;
        uint32 totalTiles = tileRows * tileCols;
        threadCount = std::min(threadCount, totalTiles);
        if (threadCount < 1) threadCount = 1;

        dng_log(DNG_LOG_DEBUG, "[deptheos_host] PerformAreaTask: %u threads, area %dx%d, tile %dx%d",
                threadCount, area.W(), area.H(), tileSize.h, tileSize.v);

        task.Start(threadCount, area, tileSize, &Allocator(), Sniffer());

        // Each thread gets a band of whole tiles: rows when there are enough,
        // else columns.
        try {
            if (threadCount == 1) {
                task.ProcessOnThread(0, area, tileSize, Sniffer(), progress);
            } else if (tileRows >= threadCount) {
                int32 rowsPerThread = tileSize.v * ((tileRows + threadCount - 1) / threadCount);

                RunOnThreads(threadCount, [&](uint32 t) {
                    int32 top = area.t + (int32)t * rowsPerThread;
                    int32 bot = std::min(top + rowsPerThread, area.b);
                    if (top >= area.b) return;
                    dng_rect subArea(top, area.l, bot, area.r);
                    task.ProcessOnThread(t, subArea, tileSize, Sniffer(), progress);
                });
            } else {
                int32 colsPerThread = tileSize.h * ((tileCols + threadCount - 1) / threadCount);

                RunOnThreads(threadCount, [&](uint32 t) {
                    int32 left = area.l + (int32)t * colsPerThread;
                    int32 right = std::min(left + colsPerThread, area.r);
                    if (left >= area.r) return;
                    dng_rect subArea(area.t, left, area.b, right);
                    task.ProcessOnThread(t, subArea, tileSize, Sniffer(), progress);
                });
            }
        } catch (...) {
            // Finish pairs with Start on every path; the task's exception
            // wins over one from Finish.
            try {
                task.Finish(threadCount);
            } catch (...) {
            }
            throw;
        }

        task.Finish(threadCount);
    }

    // Runs body(t) for t in [0, count) on one engine thread each, joins all,
    // and rethrows the first exception. Nothing may escape a thread (that
    // terminates the process), and every started thread is joined even when
    // starting a later one fails.
    template <typename Body>
    static void RunOnThreads(uint32 count, Body &&body) {
        using Fn = std::remove_reference_t<Body>;
        struct Run {
            Fn *body;
            uint32 t;
            std::exception_ptr *firstError;
            std::mutex *errorMutex;
        };
        std::exception_ptr firstError = nullptr;
        std::mutex errorMutex;
        auto record = [&](std::exception_ptr error) {
            std::lock_guard<std::mutex> lock(errorMutex);
            if (!firstError) firstError = error;
        };
        std::vector<Run> runs(count);
        std::vector<engine_thread *> threads;
        try {
            threads.reserve(count);
            for (uint32 t = 0; t < count; ++t) {
                runs[t] = Run{&body, t, &firstError, &errorMutex};
                engine_thread *thread = start_engine_thread(
                    [](void *arg) {
                        Run &run = *static_cast<Run *>(arg);
                        try {
                            (*run.body)(run.t);
                        } catch (...) {
                            std::lock_guard<std::mutex> lock(*run.errorMutex);
                            if (!*run.firstError) *run.firstError = std::current_exception();
                        }
                    },
                    &runs[t]);
                if (!thread) throw std::runtime_error("could not start a thread");
                threads.push_back(thread);
            }
        } catch (...) {
            record(std::current_exception());
        }
        for (engine_thread *thread : threads) {
            join_engine_thread(thread);
        }
        if (firstError) {
            std::rethrow_exception(firstError);
        }
    }

private:
    uint32 fThreads;
    deptheos_sniffer fSniffer;
};

// Runs `body(top, bottom)` over row bands on the host's threads, at most
// one band per 16 MB of `bytes` written: a small image isn't worth threads.
template <typename Body>
void for_each_band(deptheos_host &host, int32 top, int32 bottom, size_t bytes, const Body &body) {
    const size_t height = static_cast<size_t>(bottom - top);
    const size_t bands = std::min<size_t>({host.PerformAreaTaskThreads(), bytes >> 24, height});
    if (bands <= 1) {
        body(top, bottom);
        return;
    }
    const int32 rows = static_cast<int32>((height + bands - 1) / bands);
    deptheos_host::RunOnThreads(static_cast<uint32>(bands), [&](uint32 band) {
        host.CheckCanceled();
        const int32 from = top + static_cast<int32>(band) * rows;
        const int32 to = std::min(from + rows, bottom);
        if (from < to) body(from, to);
    });
}

// Fast paths for the common linear DNG (dng_fast_read.cpp,
// dng_lossless_jpeg_fast.cpp), each matching the SDK pixel for pixel.

// Whether this thread's reads skip the fast paths (a test hook).
bool dng_reference_reads();

// How read_stage1_fast read stage 1. The values are the test hook's
// (deptheos_dng_last_read_path): keep them.
enum class stage1_read {
    not_read = 0,              // not read: the SDK's ReadStage1Image reads this image
    uncompressed = 1,          // uncompressed, copied on several threads
    lossless_jpeg = 2,         // lossless JPEG, every tile by decode_lossless_jpeg
    lossless_jpeg_partly = 3,  // lossless JPEG, some tiles (or all) by the SDK's decoder
};

// For the test hook.
void dng_record_read_path(stage1_read stage1, bool deferredBlackLevel);

// One lossless JPEG tile, exactly the SDK's `samples` samples, into `out`.
// False (`out` undefined) for a stream it doesn't handle, left to the SDK.
bool decode_lossless_jpeg(const uint8_t *data, size_t size, uint16_t *out, size_t samples);

// Stage 1 of two kinds of image: uncompressed interleaved native-endian
// 16-bit (positional reads from `path` on several threads), and lossless
// JPEG (decode_lossless_jpeg where it can). Any other is left for the SDK.
// `bypassCache` goes around the OS file cache (Windows only).
stage1_read read_stage1_fast(deptheos_host &host, dng_stream &stream, dng_info &info,
                             dng_negative &negative, const std::string &path,
                             bool bypassCache);

// Copies `crop` (stage 2 coordinates) of a deferred negative's stage 2 into
// `out` through the per-plane tables.
void copy_out_linearized(deptheos_host &host, const deptheos_negative &negative,
                         const dng_rect &crop, uint32 planes, uint16_t *out);

void fill_image_metadata(dng_host &host, dng_negative &negative,
                         DngImageMetadata &out);


// Stage 3's default crop rendered by the SDK to 16-bit sRGB (gray gamma 2.2
// for a monochrome negative), at `white` (none: as shot) and `exposure`
// stops over the baseline.
DngReadResult render_negative_to_srgb(dng_host &host, dng_negative &negative,
                                      const std::optional<DngWhiteXY> &white = std::nullopt,
                                      double exposure = 0.0);

dng_negative * rebuild_negative_from_metadata(
    dng_host &host, const DngImageMetadata &meta,
    const uint16_t *pixels, int w, int h, int ch);

// ---------------------------------------------------------------------------
// Files. Paths arrive as UTF-8; Windows uses the wide-character calls, since
// its narrow ones go through the ANSI code page.
// ---------------------------------------------------------------------------

#if qWinOS
std::wstring widen_utf8(const std::string &utf8);
#endif

// A dng_file_stream on a UTF-8 path.
class utf8_file_stream : public dng_file_stream {
public:
    explicit utf8_file_stream(const std::string &path, bool output = false)
#if qWinOS
        : dng_file_stream(widen_utf8(path).c_str(), output) {}
#else
        : dng_file_stream(path.c_str(), output) {}
#endif
};

