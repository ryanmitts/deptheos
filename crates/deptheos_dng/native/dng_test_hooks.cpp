// The test hooks declared in dng_bridge_testing.h.
#include "dng_bridge_internal.h"
#include "dng_bridge_testing.h"

#include "dng_bottlenecks.h"
#include "dng_lossless_jpeg.h"

#include <cstring>

namespace {

class vector_spooler : public dng_spooler {
public:
    std::vector<uint16_t> fSamples;
    void Spool(const void *data, uint32 count) override {
        const size_t at = fSamples.size();
        fSamples.resize(at + count / 2);
        std::memcpy(fSamples.data() + at, data, count / 2 * 2);
    }
};

} // namespace

std::vector<uint8_t> deptheos_dng_encode_lossless_jpeg(const uint16_t *samples, uint32_t rows,
                                                       uint32_t cols, uint32_t channels,
                                                       uint32_t bits) {
    dng_memory_stream stream(gDefaultDNGMemoryAllocator);
    DoEncodeLosslessJPEG(samples, rows, cols, channels, bits,
                         static_cast<int32>(cols * channels), static_cast<int32>(channels),
                         stream);
    std::vector<uint8_t> out(static_cast<size_t>(stream.Length()));
    stream.SetReadPosition(0);
    stream.Get(out.data(), static_cast<uint32>(out.size()));
    return out;
}

int deptheos_dng_compare_lossless_jpeg(const uint8_t *data, size_t size, size_t samples) {
    std::vector<uint16_t> fast(samples);
    if (!decode_lossless_jpeg(data, size, fast.data(), samples)) return 0;
    vector_spooler spooler;
    try {
        dng_stream stream(data, static_cast<uint32>(size));
        const uint32 bytes = static_cast<uint32>(samples * 2);
        DoDecodeLosslessJPEG(stream, spooler, bytes, bytes, false, size);
    } catch (...) {
        return -1;
    }
    if (spooler.fSamples.size() < samples) return -1;
    spooler.fSamples.resize(samples);
    return spooler.fSamples == fast ? 1 : -2;
}

std::vector<uint16_t> deptheos_dng_render_file_srgb(const std::string &path,
                                                    const std::string &dcp_path, uint32_t x,
                                                    uint32_t y, uint32_t w, uint32_t h) {
    deptheos_host host;
    utf8_file_stream stream(path);
    dng_info info;
    info.Parse(host, stream);
    info.PostParse(host);
    if (!info.IsValidDNG()) ThrowBadFormat();

    AutoPtr<dng_negative> negative(host.Make_dng_negative());
    negative->Parse(host, stream, info);
    negative->PostParse(host, stream, info);
    if (negative->GetMosaicInfo() || !(negative->BaseOrientation() == dng_orientation::Normal())) {
        ThrowNotYetImplemented();
    }
    negative->ReadStage1Image(host, stream, info);
    negative->BuildStage2Image(host);
    // Opcode list 3 stays out of stage 3, as the bridge's read leaves it out
    // of the pixels.
    dng_opcode_list list3(3);
    list3.Swap(negative->OpcodeList3());
    negative->BuildStage3Image(host);

    if (!dcp_path.empty()) {
        utf8_file_stream dcp(dcp_path);
        AutoPtr<dng_camera_profile> profile(new dng_camera_profile);
        if (!profile->ParseExtended(dcp)) ThrowBadFormat();
        negative->ClearProfiles();
        negative->AddProfile(profile);
    }

    // The crop, placed as the bridge's read places the default crop.
    const uint32 ox = static_cast<uint32>(negative->DefaultCropOriginH().As_real64() + 0.5);
    const uint32 oy = static_cast<uint32>(negative->DefaultCropOriginV().As_real64() + 0.5);
    negative->SetDefaultCropOrigin(ox + x, oy + y);
    negative->SetDefaultCropSize(w, h);

    DngReadResult rendered = render_negative_to_srgb(host, *negative);
    if (rendered.channels != 3 || rendered.width != static_cast<int32_t>(w) ||
        rendered.height != static_cast<int32_t>(h)) {
        ThrowProgramError("render_file_srgb: unexpected render size");
    }
    return std::move(rendered.pixels);
}

DngFrameTags deptheos_dng_frame_of(const std::string &path) {
    deptheos_host host;
    utf8_file_stream stream(path);
    dng_info info;
    info.Parse(host, stream);
    info.PostParse(host);
    if (!info.IsValidDNG()) ThrowBadFormat();
    AutoPtr<dng_negative> negative(host.Make_dng_negative());
    negative->Parse(host, stream, info);
    negative->PostParse(host, stream, info);
    DngFrameTags frame;
    frame.userCrop[0] = negative->DefaultUserCropT().As_real64();
    frame.userCrop[1] = negative->DefaultUserCropL().As_real64();
    frame.userCrop[2] = negative->DefaultUserCropB().As_real64();
    frame.userCrop[3] = negative->DefaultUserCropR().As_real64();
    frame.orientation = negative->BaseOrientation().GetTIFF();
    return frame;
}
