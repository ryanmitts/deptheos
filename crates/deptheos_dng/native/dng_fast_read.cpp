// Faster equivalents of three SDK read steps for the linear DNGs a
// converter writes (16-bit, interleaved, uncompressed or lossless JPEG).
// Each gives the SDK's own pixels and steps aside for anything else.
//
// - Stage 1, uncompressed: positional reads on 8 threads instead of the
//   SDK's one (a 270 MB file from the OS cache: about 20 ms against 50).
// - Stage 1, lossless JPEG: dng_lossless_jpeg_fast.cpp decodes the tiles it
//   recognizes, inside the SDK's own loop and checks.
// - Stage 2: the black-level pass, a memory trip per plane, waits for the
//   copy out, which reads the frame anyway.
#include "dng_bridge_internal.h"
#include "dng_bridge_testing.h"

#include "dng_ifd.h"
#include "dng_linearization_info.h"
#include "dng_read_image.h"
#include "dng_utils.h"

#include <atomic>
#include <cstring>
#include <memory>

#if qWinOS
#include <windows.h>
#else
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

// What this file relies on in the SDK, to re-check on an SDK update:
// - dng_read_image::Read validates tile offsets and byte counts, then reads
//   each lossless JPEG tile through the virtual ReadTile -> ReadLosslessJPEG,
//   with the stream at the tile's start; its spooler's only extras are
//   sub-tile reordering and the Bug16 variant.
// - dng_image::Put keeps the part of a buffer inside the image.
// - ReadStage1Image's steps beyond reading and opcode lists apply only to
//   lossy JPEG, JPEG XL and float images.
// - dng_linearize_plane's single-table formula (DoBuildStage2), and
//   BuildStage2Image doing nothing more to stage 2 after DoBuildStage2 when
//   opcode list 2 is empty and no DNG save is pending.
// `override` catches signature changes; fast_read_tests.rs catches the SDK
// skipping these hooks and any pixel that differs.
static_assert(dngVersion_Current == dngVersion_1_7_1_0,
              "SDK updated: re-check dng_fast_read.cpp against it (see above)");

static thread_local bool g_reference_reads = false;

void deptheos_dng_set_reference_reads(bool on) { g_reference_reads = on; }

bool dng_reference_reads() { return g_reference_reads; }

static thread_local DngReadPath g_last_read_path;

void dng_record_read_path(stage1_read stage1, bool deferredBlackLevel) {
    g_last_read_path = {static_cast<int32_t>(stage1), deferredBlackLevel};
}

DngReadPath deptheos_dng_last_read_path() { return g_last_read_path; }

// ---------------------------------------------------------------------------
// Stage 1
// ---------------------------------------------------------------------------

namespace {

// A read-only file with reads at an offset. Each reading thread opens its
// own: on Windows a synchronous handle serializes its ReadFile calls.
//
// With `bypassCache`, on Windows, reads go around the OS file cache
// (FILE_FLAG_NO_BUFFERING): a stack too big for free memory otherwise
// fills the cache and every read waits on eviction (a 6.7 GB/s SSD read at
// about 2 GB/s). Opening a file this way also drops its cached pages.
// Elsewhere the flag does nothing.
class positional_file {
public:
    positional_file(const std::string &path, bool bypassCache) {
#if qWinOS
        fUnbuffered = bypassCache;
        fHandle = CreateFileW(widen_utf8(path).c_str(), GENERIC_READ,
                              FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                              OPEN_EXISTING,
                              fUnbuffered ? FILE_FLAG_NO_BUFFERING : FILE_ATTRIBUTE_NORMAL,
                              nullptr);
        if (fHandle == INVALID_HANDLE_VALUE) ThrowOpenFile();
#else
        (void)bypassCache;
        fFd = ::open(path.c_str(), O_RDONLY | O_CLOEXEC);
        if (fFd < 0) ThrowOpenFile();
#endif
    }

    ~positional_file() {
#if qWinOS
        if (fBounce) VirtualFree(fBounce, 0, MEM_RELEASE);
        CloseHandle(fHandle);
#else
        ::close(fFd);
#endif
    }

    positional_file(const positional_file &) = delete;
    positional_file &operator=(const positional_file &) = delete;

    uint64 Size() const {
#if qWinOS
        LARGE_INTEGER size;
        if (!GetFileSizeEx(fHandle, &size)) ThrowReadFile();
        return static_cast<uint64>(size.QuadPart);
#else
        struct stat st;
        if (::fstat(fFd, &st) != 0) ThrowReadFile();
        return static_cast<uint64>(st.st_size);
#endif
    }

    // Exactly `count` bytes at `offset`, or a dng_exception.
    void Read(uint64 offset, void *data, uint32 count) {
#if qWinOS
        if (fUnbuffered) return ReadUnbuffered(offset, data, count);
#endif
        char *p = static_cast<char *>(data);
        while (count > 0) {
#if qWinOS
            OVERLAPPED at = {};
            at.Offset = static_cast<DWORD>(offset);
            at.OffsetHigh = static_cast<DWORD>(offset >> 32);
            DWORD got = 0;
            if (!ReadFile(fHandle, p, count, &got, &at) || got == 0) ThrowReadFile();
#else
            const ssize_t got = ::pread(fFd, p, count, static_cast<off_t>(offset));
            if (got < 0 && errno == EINTR) continue;
            if (got <= 0) ThrowReadFile();
#endif
            p += got;
            offset += static_cast<uint64>(got);
            count -= static_cast<uint32>(got);
        }
    }

private:
#if qWinOS
    // Unbuffered reads need sector alignment, so this goes through a
    // page-aligned bounce buffer.
    void ReadUnbuffered(uint64 offset, void *data, uint32 count) {
        constexpr uint64 kAlign = 4096;
        const uint64 start = offset & ~(kAlign - 1);
        const uint64 end = (offset + count + kAlign - 1) & ~(kAlign - 1);
        const uint64 span = end - start;
        const uint64 need = offset + count - start;
        if (span > 0xFFFFFFFFull) ThrowReadFile();
        if (fBounceSize < span) {
            if (fBounce) VirtualFree(fBounce, 0, MEM_RELEASE);
            fBounce = static_cast<char *>(
                VirtualAlloc(nullptr, span, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
            if (!fBounce) ThrowMemoryFull();
            fBounceSize = span;
        }
        uint64 have = 0;
        while (have < need) {
            OVERLAPPED at = {};
            const uint64 at_offset = start + have;
            at.Offset = static_cast<DWORD>(at_offset);
            at.OffsetHigh = static_cast<DWORD>(at_offset >> 32);
            DWORD got = 0;
            if (!ReadFile(fHandle, fBounce + have, static_cast<DWORD>(span - have), &got, &at) ||
                got == 0) {
                ThrowReadFile();
            }
            have += got;
        }
        memcpy(data, fBounce + (offset - start), count);
    }

    HANDLE fHandle;
    bool fUnbuffered = false;
    char *fBounce = nullptr;
    uint64 fBounceSize = 0;
#else
    int fFd;
#endif
};

// Each tile's (or strip's) offset, as dng_read_image::Read finds them.
std::vector<uint64> tile_offsets(const dng_ifd &ifd, dng_stream &stream, uint32 count) {
    std::vector<uint64> offsets(count);
    if (count <= dng_ifd::kMaxTileInfo) {
        for (uint32 i = 0; i < count; i++) offsets[i] = ifd.fTileOffset[i];
    } else {
        stream.SetReadPosition(ifd.fTileOffsetsOffset);
        for (uint32 i = 0; i < count; i++) offsets[i] = stream.TagValue_uint64(ifd.fTileOffsetsType);
    }
    return offsets;
}

// The SDK's tile reader with decode_lossless_jpeg tried before its own
// decoder. The SDK's Read still validates the tiles and runs the loop.
class lossless_jpeg_reader : public dng_read_image {
public:
    std::atomic<uint32> fFast{0};
    std::atomic<uint32> fDeclined{0};

protected:
    bool ReadLosslessJPEG(dng_host &host, const dng_ifd &ifd, dng_stream &stream,
                          dng_image &image, const dng_rect &tileArea, uint32 plane,
                          uint32 planes, uint32 tileByteCount,
                          AutoPtr<dng_memory_block> &uncompressedBuffer,
                          AutoPtr<dng_memory_block> &subTileBlockBuffer) override {
        // A cancel stops the read between tiles.
        dng_abort_sniffer::SniffForAbort(host.Sniffer());
        // The decoded tile is held whole, so only tiles up to this size (a
        // converter's are 138 KB).
        constexpr uint64 kMaxTileBytes = 16 << 20;
        const uint64 bytes = static_cast<uint64>(tileArea.W()) * tileArea.H() * planes * 2;
        // The SDK's spooler also reorders sub-tile blocks and reads the
        // fLosslessJPEGBug16 variant; this decoder does neither.
        if (!tileArea.IsEmpty() && bytes <= kMaxTileBytes && !ifd.fLosslessJPEGBug16 &&
            ifd.fSubTileBlockRows <= 1 && ifd.fSubTileBlockCols <= 1) {
            const uint64 start = stream.Position();
            // A memory stream on several threads; the file's stream on one.
            const uint8_t *data = nullptr;
            std::vector<uint8_t> copy;
            if (const void *all = stream.Data();
                all && tileByteCount <= stream.Length() - start) {
                data = static_cast<const uint8_t *>(all) + start;
            } else {
                copy.resize(tileByteCount);
                stream.Get(copy.data(), tileByteCount);
                data = copy.data();
            }
            // The SDK's buffer for this thread, as its own decode uses.
            if (!uncompressedBuffer.Get() || uncompressedBuffer->LogicalSize() < bytes) {
                uncompressedBuffer.Reset(host.Allocate(static_cast<uint32>(bytes)));
            }
            uint16_t *samples = uncompressedBuffer->Buffer_uint16();
            if (decode_lossless_jpeg(data, tileByteCount, samples, bytes / 2)) {
                // As the SDK's spooler puts the decoded rows.
                dng_pixel_buffer buffer(tileArea, plane, planes, ttShort, pcInterleaved, samples);
                image.Put(buffer);
                stream.SetReadPosition(start + tileByteCount);
                fFast.fetch_add(1, std::memory_order_relaxed);
                return true;
            }
            stream.SetReadPosition(start);
        }
        fDeclined.fetch_add(1, std::memory_order_relaxed);
        return dng_read_image::ReadLosslessJPEG(host, ifd, stream, image, tileArea, plane,
                                                planes, tileByteCount, uncompressedBuffer,
                                                subTileBlockBuffer);
    }
};

// A lossless JPEG image (16-bit integer samples), read as the SDK's
// ReadStage1Image would; its other steps apply only to lossy, JPEG XL and
// float images.
stage1_read read_lossless_jpeg(deptheos_host &host, dng_stream &stream, dng_info &info,
                               dng_negative &negative) {
    const dng_ifd &ifd = *info.fIFD[info.fMainIndex];
    if (ifd.fCompression != ccJPEG || ifd.IsBaselineJPEG() || ifd.PixelType() != ttShort) {
        return stage1_read::not_read;
    }
    AutoPtr<dng_image> image(host.Make_dng_image(ifd.Bounds(), ifd.fSamplesPerPixel, ttShort));
    lossless_jpeg_reader reader;
    reader.Read(host, ifd, stream, *image, nullptr, nullptr);
    negative.SetStage1Image(image);
    negative.ReadOpcodeLists(host, stream, info);
    const uint32 declined = reader.fDeclined.load();
    if (declined == 0) return stage1_read::lossless_jpeg;
    dng_log(DNG_LOG_DEBUG, "[read_dng] %u of %u lossless JPEG tiles left to the SDK", declined,
            declined + reader.fFast.load());
    return stage1_read::lossless_jpeg_partly;
}

stage1_read read_uncompressed(deptheos_host &host, dng_stream &stream, dng_info &info,
                              dng_negative &negative, const std::string &path,
                              bool bypassCache) {
    const dng_ifd &ifd = *info.fIFD[info.fMainIndex];

    // Tiles the SDK's ReadUncompressed copies as they are: interleaved
    // 16-bit samples in this machine's byte order.
    const uint32 planes = ifd.fSamplesPerPixel;
    if (ifd.fCompression != ccUncompressed || ifd.fPlanarConfiguration != pcInterleaved ||
        planes < 1 || planes > kMaxColorPlanes || ifd.fSampleBitShift != 0 ||
        ifd.fSubTileBlockRows > 1 || ifd.fSubTileBlockCols > 1 ||
        ifd.fRowInterleaveFactor > 1 || ifd.fColumnInterleaveFactor > 1 || stream.SwapBytes() ||
        ifd.fImageWidth > 2 * kMaxImageSide || ifd.fImageLength > 2 * kMaxImageSide) {
        return stage1_read::not_read;
    }
    for (uint32 p = 0; p < planes; p++) {
        if (ifd.fBitsPerSample[p] != 16 || ifd.fSampleFormat[p] != sfUnsignedInteger) {
            return stage1_read::not_read;
        }
    }

    const uint32 across = ifd.TilesAcross();
    const uint32 down = ifd.TilesDown();
    const uint32 count = across * down;
    // The division catches a tile count that overflowed.
    if (across == 0 || down == 0 || count / across != down) return stage1_read::not_read;

    // Every tile must lie inside the file, its size from the geometry as the
    // SDK takes it (TileByteCounts is not read).
    std::vector<uint64> offsets = tile_offsets(ifd, stream, count);
    for (uint32 i = 0; i < count; i++) {
        const dng_rect area = ifd.TileArea(i / across, i % across);
        const uint64 bytes = static_cast<uint64>(area.W()) * area.H() * planes * 2;
        if (area.IsEmpty() || bytes != ifd.TileByteCount(area) ||
            offsets[i] > stream.Length() || bytes > stream.Length() - offsets[i]) {
            return stage1_read::not_read;
        }
    }

    // The threads reopen the file by path; if that fails, or its size
    // differs from the one parsed, the SDK reads it.
    try {
        positional_file probe(path, bypassCache);
        if (probe.Size() != stream.Length()) return stage1_read::not_read;
    } catch (...) {
        return stage1_read::not_read;
    }

    AutoPtr<dng_image> image(host.Make_dng_image(ifd.Bounds(), planes, ttShort));
    const dng_rect bounds = image->Bounds();

    // Runs of whole tile rows, about 2 MB each: a converter often writes
    // one tile for the whole frame.
    struct Run {
        uint64 offset;
        dng_rect area;
    };
    constexpr uint64 kRunBytes = 2 << 20;
    std::vector<Run> runs;
    uint64 largest = 0;
    for (uint32 i = 0; i < count; i++) {
        const dng_rect area = ifd.TileArea(i / across, i % across);
        const uint64 rowBytes = static_cast<uint64>(area.W()) * planes * 2;
        const int32 rowsPerRun = static_cast<int32>(std::max<uint64>(1, kRunBytes / rowBytes));
        // Rows past the image's bottom edge are padding.
        const int32 bottom = std::min(area.b, bounds.b);
        for (int32 top = area.t; top < bottom; top += rowsPerRun) {
            const dng_rect run(top, area.l, std::min(top + rowsPerRun, bottom), area.r);
            runs.push_back({offsets[i] + static_cast<uint64>(top - area.t) * rowBytes, run});
            largest = std::max<uint64>(largest, rowBytes * run.H());
        }
    }
    if (runs.empty() || largest > 0xFFFFFFFFull) return stage1_read::not_read;

    // A run spanning the image's rows is laid out as in memory, so it is
    // read straight into the image.
    dng_dirty_tile_buffer whole(*image, bounds);
    const bool rowsContiguous = whole.fColStep == static_cast<int32>(planes) &&
                                whole.fPlaneStep == 1 &&
                                whole.fRowStep == static_cast<int32>(bounds.W() * planes);

    // Flat beyond 8 threads, and a stack runs a second decode and the GPU
    // alongside this one.
    const uint32 threads = static_cast<uint32>(
        std::min<size_t>({host.PerformAreaTaskThreads(), 8u, runs.size()}));
    std::atomic<size_t> next{0};
    deptheos_host::RunOnThreads(threads, [&](uint32) {
        positional_file file(path, bypassCache);
        std::unique_ptr<uint16[]> scratch;
        for (size_t i; (i = next.fetch_add(1, std::memory_order_relaxed)) < runs.size();) {
            host.CheckCanceled();
            const Run &run = runs[i];
            const uint32 bytes = static_cast<uint32>(static_cast<uint64>(run.area.W()) *
                                                     run.area.H() * planes * 2);
            if (rowsContiguous && run.area.l == bounds.l && run.area.r == bounds.r) {
                file.Read(run.offset, whole.DirtyPixel_uint16(run.area.t, run.area.l, 0), bytes);
                continue;
            }
            // Otherwise through a scratch buffer: a tile past the right edge
            // is padded, and Put keeps what lies inside.
            if (!scratch) scratch.reset(new uint16[largest / 2]);
            file.Read(run.offset, scratch.get(), bytes);
            dng_pixel_buffer buffer(run.area, 0, planes, ttShort, pcInterleaved, scratch.get());
            image->Put(buffer);
        }
    });

    negative.SetStage1Image(image);
    // As the SDK's ReadStage1Image ends.
    negative.ReadOpcodeLists(host, stream, info);
    return stage1_read::uncompressed;
}

} // namespace

stage1_read read_stage1_fast(deptheos_host &host, dng_stream &stream, dng_info &info,
                             dng_negative &negative, const std::string &path,
                             bool bypassCache) {
    if (dng_reference_reads()) return stage1_read::not_read;
    const stage1_read read = read_uncompressed(host, stream, info, negative, path, bypassCache);
    if (read != stage1_read::not_read) return read;
    return read_lossless_jpeg(host, stream, info, negative);
}

// ---------------------------------------------------------------------------
// Stage 2
// ---------------------------------------------------------------------------

dng_negative *deptheos_negative::Make(dng_host &host) {
    AutoPtr<deptheos_negative> result(new deptheos_negative(host));
    result->Initialize();
    return result.Release();
}

void deptheos_negative::DoBuildStage2(dng_host &host) {
    fDeferred = false;
    const dng_image &stage1 = *fStage1Image.Get();
    const dng_linearization_info &info = *fLinearizationInfo.Get();
    const uint32 planes = stage1.Planes();

    // The case dng_linearize_plane serves with one table per plane: integer
    // samples, one black level per plane, no linearization table, an empty
    // opcode list 2, no preserved black level (the base negative never
    // preserves one), and no DNG save pending (which keeps stage 2 raw).
    const bool simple = fDeferLinearization && host.SaveDNGVersion() == dngVersion_None &&
                        !GetMosaicInfo() && stage1.PixelType() == ttShort &&
                        !info.fLinearizationTable.Get() &&
                        planes >= 1 && planes <= kMaxColorPlanes &&
                        info.fBlackLevelRepeatRows == 1 && info.fBlackLevelRepeatCols == 1 &&
                        !info.fBlackDeltaH.Get() && !info.fBlackDeltaV.Get() &&
                        fOpcodeList2.Count() == 0 && !SupportsPreservedBlackLevels(host) &&
                        (info.fActiveArea & stage1.Bounds()) == info.fActiveArea &&
                        !info.fActiveArea.IsEmpty();
    if (!simple) {
        dng_negative::DoBuildStage2(host);
        return;
    }

    // dng_linearize_plane's table, step for step (dng_linearization_info.cpp).
    const uint16 dstBlackLevel = Stage3BlackLevel();
    for (uint32 plane = 0; plane < planes; plane++) {
        const real64 minRange = info.fWhiteLevel[plane] - info.MaxBlackLevel(plane);
        if (minRange <= 0.0) ThrowBadFormat();
        const real64 scale = 1.0 / minRange;
        std::vector<uint16> &lut = fTables[plane];
        lut.resize(0x10000);
        for (uint32 j = 0; j < 0x10000; j++) {
            real64 y = j - info.fBlackLevel[0][0][plane];
            y *= scale;
            y = Pin_real64(0.0, y, 1.0);
            lut[j] = Pin_uint16(Round_int32(y * (0x0FFFF - dstBlackLevel) + dstBlackLevel));
        }
    }

    // Stage 2 is the active area of stage 1; the copy out reads it there.
    fDeferredOrigin = info.fActiveArea.TL();
    fDeferredArea = info.fActiveArea;
    fStage2Image.Reset(fStage1Image.Release());
    fDeferred = true;
}

void copy_out_linearized(deptheos_host &host, const deptheos_negative &negative,
                         const dng_rect &crop, uint32 planes, uint16_t *out) {
    const dng_image &image = *negative.Stage2Image();
    const dng_rect area = crop + negative.DeferredOrigin();
    const dng_rect valid = area & negative.DeferredArea();
    const size_t rowSamples = static_cast<size_t>(area.W()) * planes;
    const size_t bytes = rowSamples * area.H() * sizeof(uint16_t);
    // Outside stage 2 the SDK's copy leaves zeros (the buffer was cleared).
    if (valid != area) std::memset(out, 0, bytes);
    if (valid.IsEmpty()) return;

    const uint16 *lut[kMaxColorPlanes] = {};
    for (uint32 p = 0; p < planes; p++) lut[p] = negative.DeferredTable(p);

    auto convert = [&](int32 top, int32 bottom) {
        const dng_rect band(top, valid.l, bottom, valid.r);
        dng_const_tile_buffer src(image, band);
        const int32 colStep = src.fColStep;
        const int32 planeStep = src.fPlaneStep;
        const uint32 width = static_cast<uint32>(valid.W());
        for (int32 row = top; row < bottom; row++) {
            const uint16 *s = src.ConstPixel_uint16(row, valid.l, 0);
            uint16_t *d = out + static_cast<size_t>(row - area.t) * rowSamples +
                          static_cast<size_t>(valid.l - area.l) * planes;
            if (planes == 3 && colStep == 3 && planeStep == 1) {
                const uint16 *r = lut[0], *g = lut[1], *b = lut[2];
                for (uint32 x = 0; x < width; x++, s += 3, d += 3) {
                    d[0] = r[s[0]];
                    d[1] = g[s[1]];
                    d[2] = b[s[2]];
                }
            } else {
                for (uint32 x = 0; x < width; x++, s += colStep, d += planes) {
                    for (uint32 p = 0; p < planes; p++) d[p] = lut[p][s[p * planeStep]];
                }
            }
        }
    };

    const size_t validBytes = static_cast<size_t>(valid.W()) * valid.H() * planes * sizeof(uint16_t);
    for_each_band(host, valid.t, valid.b, validBytes, convert);
}
