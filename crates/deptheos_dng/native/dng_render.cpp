#include "dng_bridge_internal.h"

// ---------------------------------------------------------------------------
// Internal: render a negative (with Stage3 set) to sRGB 16-bit output
// ---------------------------------------------------------------------------

DngReadResult render_negative_to_srgb(dng_host &host, dng_negative &negative) {
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

DngReadResult deptheos_render_to_srgb(
        const uint16_t *pixels, size_t /* pixel_count */,
        int32_t width, int32_t height, int32_t channels,
        const DngImageMetadata &meta) {
    deptheos_host host;

    AutoPtr<dng_negative> negative(rebuild_negative_from_metadata(
        host, meta, pixels, width, height, channels));

    return render_negative_to_srgb(host, *negative);
}

