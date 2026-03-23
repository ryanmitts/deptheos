#include "dng_bridge_internal.h"

// ---------------------------------------------------------------------------
// Internal: render a negative (with Stage3 set) to sRGB 16-bit output
// ---------------------------------------------------------------------------

static DngReadResult render_negative_to_srgb(dng_host &host, dng_negative &negative) {
    dng_render render(host, negative);
    render.SetFinalSpace(negative.IsMonochrome()
        ? dng_space_GrayGamma22::Get() : dng_space_sRGB::Get());
    render.SetFinalPixelType(ttShort);

    AutoPtr<dng_image> rendered(render.Render());

    DngReadResult result;
    result.width    = rendered->Width();
    result.height   = rendered->Height();
    result.channels = rendered->Planes();

    const size_t total = static_cast<size_t>(result.width) * result.height * result.channels;
    result.pixels.resize(total);

    dng_pixel_buffer pb;
    pb.fArea      = rendered->Bounds();
    pb.fPlane     = 0;
    pb.fPlanes    = result.channels;
    pb.fPixelType = ttShort;
    pb.fPixelSize = TagTypeSize(ttShort);
    pb.fRowStep   = result.width * result.channels;
    pb.fColStep   = result.channels;
    pb.fData      = result.pixels.data();
    rendered->Get(pb);

    return result;
}

// ---------------------------------------------------------------------------
// Public API: render linear pixels + metadata to sRGB
// ---------------------------------------------------------------------------

std::optional<DngReadResult> deptheos_render_to_srgb(
        const uint16_t *pixels, size_t pixel_count,
        int32_t width, int32_t height, int32_t channels,
        const DngImageMetadata &meta) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0);
        host.SetMinimumSize(0);
        host.SetMaximumSize(0);

        AutoPtr<dng_negative> negative(rebuild_negative_from_metadata(
            host, meta, pixels, width, height, channels));

        return render_negative_to_srgb(host, *negative);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[render_to_srgb] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[render_to_srgb] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[render_to_srgb] Unknown exception");
        return std::nullopt;
    }
}

// ---------------------------------------------------------------------------
// Public API: validate a DNG file
// ---------------------------------------------------------------------------

bool deptheos_validate_dng(const std::string &path) {
    try {
        dng_file_stream stream(path.c_str());
        deptheos_host host;

        host.SetPreferredSize(0);
        host.SetMinimumSize(0);
        host.SetMaximumSize(0);

        dng_info info;
        info.Parse(host, stream);
        info.PostParse(host);

        if (!info.IsValidDNG()) {
            dng_log(DNG_LOG_ERROR, "[validate_dng] Not a valid DNG: %s", path.c_str());
            return false;
        }

        AutoPtr<dng_negative> negative(host.Make_dng_negative());
        negative->Parse(host, stream, info);
        negative->PostParse(host, stream, info);
        negative->ReadStage1Image(host, stream, info);

        negative->ValidateRawImageDigest(host);
        negative->SynchronizeMetadata();

        if (negative->Stage1Image()) {
            negative->BuildStage2Image(host);
        }
        if (negative->Stage2Image()) {
            negative->BuildStage3Image(host);
        }

        dng_render render(host, *negative);
        render.SetFinalSpace(negative->IsMonochrome()
            ? dng_space_GrayGamma22::Get() : dng_space_sRGB::Get());
        render.SetFinalPixelType(ttByte);
        render.SetMaximumSize(256);

        AutoPtr<dng_image> rendered(render.Render());

        if (rendered->Width() == 0 || rendered->Height() == 0) {
            dng_log(DNG_LOG_ERROR, "[validate_dng] Render produced empty image");
            return false;
        }

        dng_log(DNG_LOG_INFO, "[validate_dng] OK: %s (%dx%d, %d planes)",
                path.c_str(),
                rendered->Width(), rendered->Height(), rendered->Planes());

        return true;
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[validate_dng] DNG Exception: %d for %s",
                (int)e.ErrorCode(), path.c_str());
        return false;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[validate_dng] Standard exception: %s for %s", e.what(), path.c_str());
        return false;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[validate_dng] Unknown exception for %s", path.c_str());
        return false;
    }
}
