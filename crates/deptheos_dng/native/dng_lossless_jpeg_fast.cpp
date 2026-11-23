// A faster decoder for the lossless JPEG tiles DNG converters write, giving
// exactly the samples of the SDK's dng_lossless_decoder
// (dng_lossless_jpeg_shared.cpp) for the files it accepts, and declining
// everything else so the SDK decodes it.
//
// The SDK's decoder reads a byte at a time and looks codes up 8 bits at a
// time (about 0.6 CPU-seconds a 45 MP frame). This one refills a 64-bit
// buffer 8 bytes at a time, looks codes up kLookupBits at once, takes the
// difference's extra bits in the same step when they fit, and keeps its
// tables for the next tile.
//
// Accepted: one SOF3 frame (Huffman, lossless) of 1 to 4 components with
// distinct ids and sampling factors of 1, 2 to 16 bits; Huffman tables only
// for DC, at most 4 definitions; one scan of every component in frame
// order; predictors 0 to 7, any point transform; no restart interval; EOI at
// the end. Declined: anything else, and any stream the decode would need to
// guess at (a code no table holds, a difference longer than 16 bits, data
// that ends before the image does).
//
// The lossless_jpeg_* tests in fast_read_tests.rs compare it with the SDK's
// decoder stream by stream.
#include "dng_bridge_internal.h"

#include <array>
#include <cstdint>
#include <cstring>
#include <memory>

#if defined(_MSC_VER)
#include <stdlib.h>
#endif

namespace {

// Bits looked up at once: 10 and 11 measured best (larger tables outgrow L1).
constexpr int kLookupBits = 11;

// One Huffman table: a lookup of kLookupBits, then the canonical code
// ranges for the longer codes.
struct huffman_table {
    // For each kLookupBits prefix: the code's length in bits 0-4 (0: longer
    // than kLookupBits), the difference's bit count (the symbol) in bits 5-9,
    // and when code and difference fit in the prefix together, bit 10 set
    // and the difference itself in bits 16-31.
    std::array<uint32_t, 1 << kLookupBits> lookup{};
    // Codes of `length` bits run from minCode to maxCode (maxCode -1: none);
    // the first one's symbol is symbols[firstIndex].
    int32_t minCode[17]{};
    int32_t maxCode[18]{};
    int32_t firstIndex[17]{};
    uint8_t symbols[256]{};
};

constexpr uint32_t kFull = 1u << 10;

inline uint64_t load_be64(const uint8_t *p) {
    uint64_t w;
    std::memcpy(&w, p, 8);
#if defined(_MSC_VER)
    return _byteswap_uint64(w);
#else
    return __builtin_bswap64(w);
#endif
}

// The difference a symbol and its extra bits stand for (F.2.2.1), as the
// SDK's HuffExtend and its 16-bit case.
inline int32_t extend(uint32_t bits, uint32_t s) {
    const int32_t x = static_cast<int32_t>(bits);
    return x < (1 << (s - 1)) ? x - (1 << s) + 1 : x;
}

// Builds `table` from a DHT's 16 counts and symbols (C.2, F.2.2.3).
bool build_table(huffman_table &table, const uint8_t *counts, const uint8_t *symbols,
                 uint32_t symbolCount) {
    std::memcpy(table.symbols, symbols, symbolCount);
    for (uint32_t i = 0; i < symbolCount; i++) {
        // A difference has 0 to 16 bits.
        if (symbols[i] > 16) return false;
    }
    int32_t code = 0;
    int32_t index = 0;
    for (int len = 1; len <= 16; len++) {
        const int32_t n = counts[len - 1];
        table.firstIndex[len] = index;
        table.minCode[len] = code;
        table.maxCode[len] = n ? code + n - 1 : -1;
        for (int32_t k = 0; k < n; k++, code++, index++) {
            // Overfull: the counts promise more codes than `len` bits hold.
            if (code >= (1 << len)) return false;
            if (len > kLookupBits) continue;
            const uint32_t s = symbols[index];
            const int spare = kLookupBits - len;
            for (int32_t fill = 0; fill < (1 << spare); fill++) {
                const uint32_t prefix = (static_cast<uint32_t>(code) << spare) | fill;
                uint32_t entry = static_cast<uint32_t>(len) | (s << 5);
                // The difference's bits follow the code in the prefix.
                if (s == 0) {
                    entry |= kFull;
                } else if (s < 16 && static_cast<int>(len + s) <= kLookupBits) {
                    const uint32_t extra = (static_cast<uint32_t>(fill) >> (spare - s)) & ((1u << s) - 1);
                    entry = static_cast<uint32_t>(len + s) | (s << 5) | kFull |
                            (static_cast<uint32_t>(static_cast<uint16_t>(extend(extra, s))) << 16);
                } else if (s == 16) {
                    // No extra bits: the difference is -32768 (the SDK
                    // without fLosslessJPEGBug16, which this decoder declines).
                    entry |= kFull | (0x8000u << 16);
                }
                table.lookup[prefix] = entry;
            }
        }
        code <<= 1;
    }
    table.maxCode[17] = INT32_MAX;
    return true;
}

// The tables a thread built lately, keyed by DHT bytes: a converter's tiles
// share a few, and building one costs as much as decoding a few rows.
class table_cache {
public:
    // A new stream: the tables it gets stay until the next one begins.
    void BeginStream() { fStream++; }

    // The table a DHT defines, or null for one this decoder declines.
    const huffman_table *Get(const uint8_t *counts, uint32_t symbolCount) {
        const uint32_t size = 16 + symbolCount;
        for (entry &e : fEntries) {
            if (e.size == size && std::memcmp(e.key, counts, size) == 0) {
                e.stream = fStream;
                return &e.table;
            }
        }
        // Replace the entry used longest ago, never one this stream holds.
        entry *victim = nullptr;
        for (entry &e : fEntries) {
            if (e.stream != fStream && (!victim || e.stream < victim->stream)) victim = &e;
        }
        if (!victim) return nullptr;
        victim->size = 0;
        victim->stream = fStream;
        victim->table = huffman_table();
        if (!build_table(victim->table, counts, counts + 16, symbolCount)) return nullptr;
        std::memcpy(victim->key, counts, size);
        victim->size = size;
        return &victim->table;
    }

private:
    // More than a stream's 4 tables (read_headers).
    static constexpr uint32_t kEntries = 8;
    struct entry {
        uint32_t size = 0;
        uint64_t stream = 0;
        uint8_t key[16 + 256];
        huffman_table table;
    };
    entry fEntries[kEntries];
    uint64_t fStream = 1;
};

// The entropy-coded bits, most significant first. Past a marker or the end
// it reads zeros, as the SDK does, counting them so the caller can decline
// a decode that used them.
class bit_reader {
public:
    bit_reader(const uint8_t *data, const uint8_t *end) : fP(data), fEnd(end) {}

    // At least 56 bits buffered. Called with fewer than 32.
    inline void Refill() {
        if (fEnd - fP >= 8) {
            const uint64_t w = load_be64(fP);
            // No 0xFF byte among the 8: no stuffing and no marker.
            const uint64_t inv = ~w;
            if (((inv - 0x0101010101010101ull) & ~inv & 0x8080808080808080ull) == 0) {
                const int k = (63 - fCount) >> 3;
                fBuffer |= (w >> (64 - 8 * k)) << (64 - fCount - 8 * k);
                fP += k;
                fCount += 8 * k;
                return;
            }
        }
        while (fCount <= 56) {
            uint32_t c = 0;
            if (fP < fEnd && fP[0] == 0xFF && (fEnd - fP < 2 || fP[1] != 0)) {
                // A marker: the entropy-coded data ends here.
                fEnd = fP;
            }
            if (fP < fEnd) {
                c = *fP++;
                if (c == 0xFF) fP++;  // the stuffed zero
            } else {
                fPadding += 8;
            }
            fBuffer |= static_cast<uint64_t>(c) << (56 - fCount);
            fCount += 8;
        }
    }

    inline uint32_t Peek(int bits) const { return static_cast<uint32_t>(fBuffer >> (64 - bits)); }

    inline void Skip(int bits) {
        fBuffer <<= bits;
        fCount -= bits;
    }

    inline int Count() const { return fCount; }

    // Whether any bit taken was padding, not data.
    bool Overran() const { return fCount < fPadding; }

private:
    const uint8_t *fP;
    const uint8_t *fEnd;
    uint64_t fBuffer = 0;
    int fCount = 0;
    int fPadding = 0;
};

// One difference. Needs 32 bits buffered; sets `ok` false for a code no
// table holds.
inline int32_t decode_difference(bit_reader &bits, const huffman_table &table, bool &ok) {
    const uint32_t entry = table.lookup[bits.Peek(kLookupBits)];
    if (entry & kFull) {
        bits.Skip(entry & 31);
        return static_cast<int16_t>(entry >> 16);
    }
    uint32_t s;
    const uint32_t len = entry & 31;
    if (len) {
        bits.Skip(static_cast<int>(len));
        s = (entry >> 5) & 31;
    } else {
        // A code longer than kLookupBits.
        int l = kLookupBits + 1;
        int32_t code = static_cast<int32_t>(bits.Peek(l));
        while (code > table.maxCode[l]) {
            if (++l > 16) {
                ok = false;
                return 0;
            }
            code = static_cast<int32_t>(bits.Peek(l));
        }
        bits.Skip(l);
        s = table.symbols[table.firstIndex[l] + (code - table.minCode[l])];
        if (s == 0) return 0;
        if (s == 16) return -32768;
    }
    const uint32_t extra = bits.Peek(static_cast<int>(s));
    bits.Skip(static_cast<int>(s));
    return extend(extra, s);
}

struct frame_info {
    uint32_t precision = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t components = 0;
    uint8_t ids[4]{};
    uint32_t predictor = 0;
    uint32_t pointTransform = 0;
    const huffman_table *tables[4]{};
};

inline uint32_t be16(const uint8_t *p) { return (static_cast<uint32_t>(p[0]) << 8) | p[1]; }

// The headers up to the scan's data, as the SDK's ReadFileHeader and
// ReadScanHeader read them. Returns where the data starts, or null.
const uint8_t *read_headers(const uint8_t *p, const uint8_t *end, frame_info &frame,
                            table_cache &cache) {
    if (end - p < 2 || p[0] != 0xFF || p[1] != 0xD8) return nullptr;
    p += 2;
    bool haveFrame = false;
    const huffman_table *tables[4] = {};
    uint32_t defined = 0;
    while (true) {
        // Fill bytes may come before a marker.
        while (p < end && *p == 0xFF && p + 1 < end && p[1] == 0xFF) p++;
        if (end - p < 4 || p[0] != 0xFF) return nullptr;
        const uint8_t marker = p[1];
        const uint32_t length = be16(p + 2);
        if (length < 2 || static_cast<size_t>(end - p - 2) < length) return nullptr;
        const uint8_t *seg = p + 4;
        const uint8_t *next = p + 2 + length;
        const uint32_t size = length - 2;
        switch (marker) {
        case 0xC3: {  // SOF3
            if (haveFrame || size < 6) return nullptr;
            frame.precision = seg[0];
            frame.height = be16(seg + 1);
            frame.width = be16(seg + 3);
            frame.components = seg[5];
            if (frame.precision < 2 || frame.precision > 16 || frame.width == 0 ||
                frame.height == 0 || frame.components < 1 || frame.components > 4 ||
                size != 6 + 3 * frame.components) {
                return nullptr;
            }
            for (uint32_t c = 0; c < frame.components; c++) {
                const uint8_t *comp = seg + 6 + 3 * c;
                if (comp[1] != 0x11) return nullptr;  // sampling factors
                frame.ids[c] = comp[0];
                // The SDK's GetSos maps repeated ids to the first such
                // component, so they would share one table.
                for (uint32_t earlier = 0; earlier < c; earlier++) {
                    if (frame.ids[earlier] == comp[0]) return nullptr;
                }
            }
            haveFrame = true;
            break;
        }
        case 0xC4: {  // DHT, one or more tables
            const uint8_t *q = seg;
            while (q < next) {
                if (next - q < 17) return nullptr;
                const uint32_t tc = q[0] >> 4, th = q[0] & 15;
                if (tc != 0 || th > 3) return nullptr;
                uint32_t count = 0;
                for (int i = 1; i <= 16; i++) count += q[i];
                if (count > 256 || static_cast<size_t>(next - q - 17) < count) return nullptr;
                // Four table ids; more definitions than that only redefine.
                if (++defined > 4) return nullptr;
                tables[th] = cache.Get(q + 1, count);
                if (!tables[th]) return nullptr;
                q += 17 + count;
            }
            break;
        }
        case 0xDD:  // DRI
            if (size != 2 || be16(seg) != 0) return nullptr;
            break;
        case 0xDA: {  // SOS
            if (!haveFrame || size < 1) return nullptr;
            const uint32_t n = seg[0];
            if (n != frame.components || size != 4 + 2 * n) return nullptr;
            for (uint32_t c = 0; c < n; c++) {
                // In frame order: the SDK's rows are in scan order.
                if (seg[1 + 2 * c] != frame.ids[c]) return nullptr;
                const uint32_t td = seg[2 + 2 * c] >> 4;
                if (td > 3 || !tables[td]) return nullptr;
                frame.tables[c] = tables[td];
            }
            frame.predictor = seg[1 + 2 * n];
            frame.pointTransform = seg[3 + 2 * n] & 15;
            if (frame.predictor > 7 || frame.pointTransform >= frame.precision) return nullptr;
            return next;
        }
        default:
            // APPn and comments are skipped; any other marker is the SDK's.
            if (!((marker >= 0xE0 && marker <= 0xEF) || marker == 0xFE)) return nullptr;
            break;
        }
        p = next;
    }
}

inline int32_t predict(uint32_t predictor, int32_t left, int32_t upper, int32_t diag) {
    switch (predictor) {
    case 0: return 0;
    case 1: return left;
    case 2: return upper;
    case 3: return diag;
    case 4: return left + upper - diag;
    case 5: return left + ((upper - diag) >> 1);
    case 6: return upper + ((left - diag) >> 1);
    default: return (left + upper) >> 1;
    }
}

// Rows of `N` interleaved components (0: any count). Predictor 1 has its
// own loop.
template <uint32_t N>
bool decode_rows(bit_reader &bits, const frame_info &frame, uint32_t rows, uint16_t *out) {
    const uint32_t comps = N ? N : frame.components;
    const uint32_t width = frame.width;
    const size_t rowSamples = static_cast<size_t>(width) * comps;
    const huffman_table *tables[4] = {frame.tables[0], frame.tables[1], frame.tables[2],
                                      frame.tables[3]};
    bool ok = true;
    for (uint32_t row = 0; row < rows; row++) {
        uint16_t *cur = out + row * rowSamples;
        const uint16_t *prev = row ? cur - rowSamples : cur;
        // The first column: from above, or mid-range in the first row.
        for (uint32_t c = 0; c < comps; c++) {
            if (bits.Count() < 32) bits.Refill();
            const int32_t d = decode_difference(bits, *tables[c], ok);
            const int32_t p = row == 0 ? 1 << (frame.precision - frame.pointTransform - 1)
                                       : prev[c];
            cur[c] = static_cast<uint16_t>(d + p);
        }
        // The first row predicts from the left whatever the predictor.
        if (row == 0 || frame.predictor == 1) {
            for (uint32_t x = 1; x < width; x++) {
                uint16_t *s = cur + x * comps;
                const uint16_t *left = s - comps;
                for (uint32_t c = 0; c < comps; c++) {
                    if (bits.Count() < 32) bits.Refill();
                    s[c] = static_cast<uint16_t>(decode_difference(bits, *tables[c], ok) + left[c]);
                }
            }
        } else {
            for (uint32_t x = 1; x < width; x++) {
                uint16_t *s = cur + x * comps;
                const uint16_t *left = s - comps;
                const uint16_t *up = prev + x * comps;
                const uint16_t *diag = up - comps;
                for (uint32_t c = 0; c < comps; c++) {
                    if (bits.Count() < 32) bits.Refill();
                    const int32_t d = decode_difference(bits, *tables[c], ok);
                    s[c] = static_cast<uint16_t>(d + predict(frame.predictor, left[c], up[c], diag[c]));
                }
            }
        }
        // Past the end of the data every sample decodes from padding.
        if (!ok || bits.Overran()) return false;
    }
    return true;
}

} // namespace

bool decode_lossless_jpeg(const uint8_t *data, size_t size, uint16_t *out, size_t samples) {
    thread_local std::unique_ptr<table_cache> cache;
    if (!cache) cache.reset(new table_cache);
    frame_info frame;
    // Ending with EOI, so the SDK's read-ahead stops at the marker instead
    // of failing at the tile's end.
    if (size < 4 || data[size - 2] != 0xFF || data[size - 1] != 0xD9) return false;
    const uint8_t *end = data + size;
    cache->BeginStream();
    const uint8_t *scan = read_headers(data, end, frame, *cache);
    if (!scan) return false;
    // The SDK requires the image to be the tile exactly.
    const size_t rowSamples = static_cast<size_t>(frame.width) * frame.components;
    if (rowSamples * frame.height != samples) return false;
    bit_reader bits(scan, end);
    switch (frame.components) {
    case 2: return decode_rows<2>(bits, frame, frame.height, out);
    case 3: return decode_rows<3>(bits, frame, frame.height, out);
    default: return decode_rows<0>(bits, frame, frame.height, out);
    }
}
