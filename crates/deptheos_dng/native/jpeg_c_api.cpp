// A JPEG decoded small through the SDK's vendored libjpeg, which scales in
// the inverse DCT (by n/8), so no full-size picture is made. For thumbnails
// of JPEGs with no usable embedded one.

#include "jpeg_c_api.h"

#include "dng_jpeglib.h"

#include <algorithm>
#include <csetjmp>
#include <limits>
#include <memory>
#include <new>
#include <vector>

struct DtJpegImage {
    std::vector<uint8_t> rgb;
    uint32_t width = 0;
    uint32_t height = 0;
};

namespace {

// libjpeg's errors longjmp back to `decode`: its default exits the process,
// and a C++ exception would have to unwind through libjpeg's C frames,
// which may be built without unwinding.
struct ErrorJump {
    jpeg_error_mgr manager;
    std::jmp_buf jump;
};

[[noreturn]] void jump_out(j_common_ptr cinfo) {
    std::longjmp(reinterpret_cast<ErrorJump *>(cinfo->err)->jump, 1);
}

void no_message(j_common_ptr) {}

// False for an error, a CMYK JPEG, or no memory. Between the setjmp and a
// longjmp only libjpeg's frames lie, and this frame holds nothing with a
// destructor (`image` is the caller's), so the jump skips no cleanup.
bool decode(const uint8_t *data, unsigned long size, uint32_t min_side, DtJpegImage &image) {
    // Zeroed, so a jump out of jpeg_create_decompress frees nothing.
    jpeg_decompress_struct cinfo{};
    ErrorJump err;
    cinfo.err = jpeg_std_error(&err.manager);
    err.manager.error_exit = jump_out;
    err.manager.output_message = no_message;
    if (setjmp(err.jump)) {
        jpeg_destroy_decompress(&cinfo);
        return false;
    }
    jpeg_create_decompress(&cinfo);
    jpeg_mem_src(&cinfo, const_cast<unsigned char *>(data), size);
    jpeg_read_header(&cinfo, TRUE);
    if (cinfo.jpeg_color_space == JCS_CMYK || cinfo.jpeg_color_space == JCS_YCCK) {
        jpeg_destroy_decompress(&cinfo);
        return false;
    }
    cinfo.out_color_space = JCS_RGB;
    // The smallest n/8 that keeps the long side at least `min_side`.
    const uint64_t side = std::max(cinfo.image_width, cinfo.image_height);
    unsigned num = 1;
    while (num < 8 && (side * num + 7) / 8 < min_side) {
        num++;
    }
    cinfo.scale_num = num;
    cinfo.scale_denom = 8;
    jpeg_start_decompress(&cinfo);
    if (cinfo.output_components != 3) {
        jpeg_destroy_decompress(&cinfo);
        return false;
    }
    image.width = cinfo.output_width;
    image.height = cinfo.output_height;
    const size_t stride = static_cast<size_t>(image.width) * 3;
    try {
        image.rgb.resize(stride * image.height);
    } catch (const std::bad_alloc &) {
        jpeg_destroy_decompress(&cinfo);
        return false;
    }
    while (cinfo.output_scanline < cinfo.output_height) {
        JSAMPROW row = image.rgb.data() + stride * cinfo.output_scanline;
        jpeg_read_scanlines(&cinfo, &row, 1);
    }
    jpeg_finish_decompress(&cinfo);
    jpeg_destroy_decompress(&cinfo);
    return true;
}

}  // namespace

extern "C" DtJpegImage *dt_jpeg_decode_scaled(const uint8_t *data, size_t size,
                                              uint32_t min_side) {
    // jpeg_mem_src takes an unsigned long, 32 bits on 64-bit Windows.
    if (!data || size == 0 || size > std::numeric_limits<unsigned long>::max()) {
        return nullptr;
    }
    std::unique_ptr<DtJpegImage> image(new (std::nothrow) DtJpegImage);
    if (!image || !decode(data, static_cast<unsigned long>(size), min_side, *image)) {
        return nullptr;
    }
    return image.release();
}

extern "C" const uint8_t *dt_jpeg_image_rgb(const DtJpegImage *image) {
    return image->rgb.data();
}

extern "C" uint32_t dt_jpeg_image_width(const DtJpegImage *image) {
    return image->width;
}

extern "C" uint32_t dt_jpeg_image_height(const DtJpegImage *image) {
    return image->height;
}

extern "C" void dt_jpeg_image_free(DtJpegImage *image) {
    delete image;
}
