#include "dng_bridge_internal.h"

// ---------------------------------------------------------------------------
// Internal: write DNG to a stream from metadata + pixels
// ---------------------------------------------------------------------------

static bool write_dng_to_stream(dng_host &host, dng_stream &outStream,
                                 const DngImageMetadata &meta,
                                 const uint16_t *pixels, size_t pixel_count,
                                 int32_t width, int32_t height, int32_t channels) {
    host.SetSaveLinearDNG(true);

    dng_date_time_info dateTimeInfo;
    CurrentDateTimeAndZone(dateTimeInfo);

    AutoPtr<dng_negative> negative(rebuild_negative_from_metadata(
        host, meta, pixels, width, height, channels));

    // Lossless JXL pre-compression (before WriteDNG)
    if (meta.geometry.useJXL) {
        host.SetSaveDNGVersion(dngVersion_Current);

        dng_jxl_encode_settings jxlSettings;
        jxlSettings.SetDistance(0.0f);  // lossless
        jxlSettings.SetEffort(7);       // default (1-9, 3=fast, 5=size, 7=default)
        jxlSettings.SetUseOriginalColorEncoding(true);
        host.SetJXLEncodeSettings(jxlSettings);

        dng_image_writer jxlWriter;
        negative->LosslessCompressJXL(host, jxlWriter);
    }

    // Previews
    dng_preview_list previewList;
    try {
        for (uint32 pi = 0; pi < 2; pi++) {
            dng_render render(host, *negative);
            render.SetFinalSpace(negative->IsMonochrome()
                ? dng_space_GrayGamma22::Get() : dng_space_sRGB::Get());
            render.SetFinalPixelType(ttByte);
            render.SetMaximumSize(pi == 0 ? 1024 : 256);
            AutoPtr<dng_image> previewImage(render.Render());

            if (pi > 0 &&
                Max_uint32(previewImage->Bounds().W(),
                           previewImage->Bounds().H()) <= 256)
                break;

            AutoPtr<dng_preview> preview((dng_preview *)new dng_jpeg_preview);
            preview->fInfo.fApplicationName.Set("Deptheos");
            preview->fInfo.fApplicationVersion.Set("1.0.0");
            preview->fInfo.fSettingsName.Set("Default");
            preview->fInfo.fColorSpace = previewImage->Planes() == 1
                ? previewColorSpace_GrayGamma22 : previewColorSpace_sRGB;
            preview->fInfo.fDateTime = dateTimeInfo.Encode_ISO_8601();

            dng_jpeg_preview *jp = dynamic_cast<dng_jpeg_preview *>(preview.Get());
            dng_image_writer pw;
            pw.EncodeJPEGPreview(host, *previewImage, *jp, pi == 0 ? 8 : 5);
            previewList.Append(preview);
        }
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_WARN, "[write_dng] Preview generation failed: %s", e.what());
    } catch (...) {
        dng_log(DNG_LOG_WARN, "[write_dng] Preview generation failed");
    }

    dng_image_writer writer;
    writer.WriteDNG(host, outStream, *negative,
                    previewList.Count() > 0 ? &previewList : nullptr,
                    dngVersion_Current, false);
    outStream.Flush();
    return true;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

bool deptheos_write_dng(const std::string &output_path,
                        const DngImageMetadata &meta,
                        const uint16_t *pixels, size_t pixel_count,
                        int32_t width, int32_t height, int32_t channels) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_file_stream outStream(output_path.c_str(), true);
        return write_dng_to_stream(host, outStream, meta, pixels, pixel_count, width, height, channels);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[write_dng] DNG Exception: %d", (int)e.ErrorCode());
        return false;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[write_dng] Standard exception: %s", e.what());
        return false;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[write_dng] Unknown exception");
        return false;
    }
}

std::optional<std::vector<uint8_t>> deptheos_write_dng_to_bytes(
        const DngImageMetadata &meta,
        const uint16_t *pixels, size_t pixel_count,
        int32_t width, int32_t height, int32_t channels) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_memory_stream memStream(host.Allocator());

        if (!write_dng_to_stream(host, memStream, meta, pixels, pixel_count, width, height, channels))
            return std::nullopt;

        size_t len = (size_t)memStream.Length();
        std::vector<uint8_t> result(len);
        memStream.SetReadPosition(0);
        memStream.Get(result.data(), (uint32)len);
        return result;
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[write_dng_to_bytes] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[write_dng_to_bytes] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[write_dng_to_bytes] Unknown exception");
        return std::nullopt;
    }
}
