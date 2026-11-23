// Writing DNGs: a negative rebuilt from metadata + pixels, with JPEG
// previews, optionally JPEG XL compressed.
#include "dng_bridge_internal.h"
#include "dng_xmp.h"

#include <cerrno>

// The app version recorded in each preview, from build.rs.
#ifndef DEPTHEOS_VERSION
#error "DEPTHEOS_VERSION must be defined (build.rs passes the crate version)"
#endif

// ---------------------------------------------------------------------------
// Internal: the reference frame's own tags
// ---------------------------------------------------------------------------

namespace {

// "YYYY:MM:DD HH:MM:SS", as EXIF writes a date, or an invalid date/time.
dng_date_time exif_date_time(const std::string &s) {
    unsigned y = 0, mo = 0, d = 0, h = 0, mi = 0, sec = 0;
    if (std::sscanf(s.c_str(), "%4u:%2u:%2u %2u:%2u:%2u", &y, &mo, &d, &h, &mi, &sec) != 6) {
        return dng_date_time();
    }
    return dng_date_time(y, mo, d, h, mi, sec);
}

// The `i`th rational; a zero denominator, which no field is written with,
// becomes 0/1 (unknown).
dng_urational urational_at(const DngSourceTag &t, size_t i) {
    const uint32 n = t.numbers[2 * i], d = t.numbers[2 * i + 1];
    return d == 0 ? dng_urational(0, 1) : dng_urational(n, d);
}

} // namespace

// Writes `tags` over `exif`'s same tags, as the JPEG and TIFF exports do. A
// tag of an unexpected kind, or a single rational with a zero denominator,
// leaves the value there.
static void apply_source_tags(dng_exif &exif, const std::vector<DngSourceTag> &tags) {
    for (const DngSourceTag &t : tags) {
        const size_t count = t.kind == DngSourceTag::Integers ? t.numbers.size()
                           : t.kind == DngSourceTag::Text     ? 0
                                                              : t.numbers.size() / 2;
        const auto text = [&](dng_string &to) {
            if (t.kind == DngSourceTag::Text) to.Set(t.text.c_str());
        };
        const auto integer = [&](uint32 &to) {
            if (t.kind == DngSourceTag::Integers && count > 0) to = t.numbers[0];
        };
        const auto urational = [&](dng_urational &to) {
            if (t.kind == DngSourceTag::Rationals && count > 0 && t.numbers[1] != 0) {
                to = dng_urational(t.numbers[0], t.numbers[1]);
            }
        };
        const auto srational = [&](dng_srational &to) {
            if (t.kind == DngSourceTag::SRationals && count > 0 && t.numbers[1] != 0) {
                to = dng_srational(static_cast<int32>(t.numbers[0]),
                                   static_cast<int32>(t.numbers[1]));
            }
        };
        const auto date = [&](dng_date_time_info &to) {
            if (t.kind != DngSourceTag::Text) return;
            const dng_date_time dt = exif_date_time(t.text);
            if (dt.IsValid()) to.SetDateTime(dt);
        };
        const auto subseconds = [&](dng_date_time_info &to) {
            if (t.kind != DngSourceTag::Text) return;
            dng_string s;
            s.Set(t.text.c_str());
            to.SetSubseconds(s);
        };
        const auto offset = [&](dng_date_time_info &to) {
            if (t.kind != DngSourceTag::Text) return;
            dng_string s;
            s.Set(t.text.c_str());
            to.SetOffsetTime(s);
        };
        switch (t.tag) {
        case tcMake: text(exif.fMake); break;
        case tcModel: text(exif.fModel); break;
        case tcArtist: text(exif.fArtist); break;
        case tcCopyright: text(exif.fCopyright); break;
        case tcExposureTime: urational(exif.fExposureTime); break;
        case tcFNumber: urational(exif.fFNumber); break;
        case tcExposureProgram: integer(exif.fExposureProgram); break;
        case tcISOSpeedRatings:
            if (t.kind == DngSourceTag::Integers) {
                for (size_t i = 0; i < 3; ++i) {
                    exif.fISOSpeedRatings[i] = i < count ? t.numbers[i] : 0;
                }
            }
            break;
        case tcSensitivityType: integer(exif.fSensitivityType); break;
        case tcRecommendedExposureIndex: integer(exif.fRecommendedExposureIndex); break;
        case tcDateTimeOriginal: date(exif.fDateTimeOriginal); break;
        case tcDateTimeDigitized: date(exif.fDateTimeDigitized); break;
        case tcOffsetTimeOriginal: offset(exif.fDateTimeOriginal); break;
        case tcOffsetTimeDigitized: offset(exif.fDateTimeDigitized); break;
        case tcSubsecTimeOriginal: subseconds(exif.fDateTimeOriginal); break;
        case tcSubsecTimeDigitized: subseconds(exif.fDateTimeDigitized); break;
        case tcShutterSpeedValue: srational(exif.fShutterSpeedValue); break;
        case tcApertureValue: urational(exif.fApertureValue); break;
        case tcExposureBiasValue: srational(exif.fExposureBiasValue); break;
        case tcMaxApertureValue: urational(exif.fMaxApertureValue); break;
        case tcMeteringMode: integer(exif.fMeteringMode); break;
        case tcLightSource: integer(exif.fLightSource); break;
        case tcFlash: integer(exif.fFlash); break;
        case tcFocalLength: urational(exif.fFocalLength); break;
        case tcExposureMode: integer(exif.fExposureMode); break;
        case tcWhiteBalance: integer(exif.fWhiteBalance); break;
        case tcFocalLengthIn35mmFilm: integer(exif.fFocalLengthIn35mmFilm); break;
        case tcSceneCaptureType: integer(exif.fSceneCaptureType); break;
        case tcLensSpecificationExif:
            if (t.kind == DngSourceTag::Rationals && count == 4) {
                for (size_t i = 0; i < 4; ++i) exif.fLensInfo[i] = urational_at(t, i);
            }
            break;
        case tcLensMakeExif: text(exif.fLensMake); break;
        case tcLensModelExif: text(exif.fLensName); break;
        default: break;
        }
    }
}

// An unknown aperture in LensInfo/LensSpecification is written 0/1, not the
// SDK's 0/0. Without both focal lengths the SDK writes neither tag.
static void known_or_zero_lens_info(dng_exif &exif) {
    if (!exif.fLensInfo[0].IsValid() || !exif.fLensInfo[1].IsValid()) return;
    for (dng_urational &r : exif.fLensInfo) {
        if (r.d == 0) r = dng_urational(0, 1);
    }
}

// ---------------------------------------------------------------------------
// Internal: write DNG to a stream from metadata + pixels
// ---------------------------------------------------------------------------

static void write_dng_to_stream(dng_host &host, dng_stream &outStream,
                                const DngImageMetadata &meta, const uint16_t *pixels,
                                int32_t width, int32_t height, int32_t channels,
                                DngWriteKind kind) {
    host.SetSaveLinearDNG(true);

    dng_date_time_info dateTimeInfo;
    CurrentDateTimeAndZone(dateTimeInfo);

    AutoPtr<dng_negative> negative(rebuild_negative_from_metadata(
        host, meta, pixels, width, height, channels));

    // A color reference keeps opcode list 3 for the size it applies to,
    // written as its original size.
    const DngOpcodeData &opcodes = meta.opcodes;
    if (kind == DngWriteKind::ColorReference && !opcodes.opcodeList3.empty() &&
        opcodes.list3Width > 0 && opcodes.list3Height > 0) {
        if (negative->OpcodeList3().IsEmpty()) {
            dng_memory_stream ms(host.Allocator());
            ms.Put(opcodes.opcodeList3.data(), (uint32)opcodes.opcodeList3.size());
            ms.SetReadPosition(0);
            negative->OpcodeList3().Parse(host, ms, (uint32)opcodes.opcodeList3.size(), 0);
        }
        negative->SetOriginalSizes(dng_point(opcodes.list3Height, opcodes.list3Width));
    }

    // The reference frame's tags over the rest, then the stack's comment and
    // XMP, which the writer merges with the XMP it derives from the EXIF.
    if (dng_exif *exif = negative->GetExif()) {
        apply_source_tags(*exif, meta.stack.sourceTags);
        known_or_zero_lens_info(*exif);
        if (!meta.stack.userComment.empty()) {
            exif->fUserComment.Set_UTF8(meta.stack.userComment.c_str());
        }
    }
#if qDNGUseXMP
    if (!meta.stack.xmp.empty() && negative->GetXMP()) {
        try {
            negative->GetXMP()->Parse(host, meta.stack.xmp.data(),
                                      static_cast<uint32>(meta.stack.xmp.size()));
        } catch (...) {
            dng_log(DNG_LOG_WARN, "[write_dng] XMP packet could not be parsed; left out");
        }
    }
#endif

    // Lossless JPEG XL: the image is compressed here, before WriteDNG.
    if (meta.geometry.useJXL) {
        host.SetSaveDNGVersion(dngVersion_Current);

        dng_jxl_encode_settings jxlSettings;
        jxlSettings.SetDistance(0.0f);  // lossless
        jxlSettings.SetEffort(7);       // the encoder's default, of 1-9
        jxlSettings.SetUseOriginalColorEncoding(true);
        host.SetJXLEncodeSettings(jxlSettings);

        dng_image_writer jxlWriter;
        negative->LosslessCompressJXL(host, jxlWriter);
    }

    // Previews
    dng_preview_list previewList;
    try {
        // A 1024 px preview, then a 256 px thumbnail unless the first was
        // already that small.
        uint32 previousMax = 0;
        for (uint32 pi = 0; pi < 2; pi++) {
            if (pi > 0 && previousMax <= 256)
                break;

            dng_render render(host, *negative);
            render.SetFinalSpace(negative->IsMonochrome()
                ? dng_space_GrayGamma22::Get() : dng_space_sRGB::Get());
            render.SetFinalPixelType(ttByte);
            render.SetMaximumSize(pi == 0 ? 1024 : 256);
            AutoPtr<dng_image> previewImage(render.Render());
            previousMax = Max_uint32(previewImage->Bounds().W(),
                                     previewImage->Bounds().H());

            AutoPtr<dng_preview> preview((dng_preview *)new dng_jpeg_preview);
            preview->fInfo.fApplicationName.Set("Deptheos");
            preview->fInfo.fApplicationVersion.Set(DEPTHEOS_VERSION);
            preview->fInfo.fSettingsName.Set("Default");
            preview->fInfo.fColorSpace = previewImage->Planes() == 1
                ? previewColorSpace_GrayGamma22 : previewColorSpace_sRGB;
            preview->fInfo.fDateTime = dateTimeInfo.Encode_ISO_8601();

            dng_jpeg_preview *jp = dynamic_cast<dng_jpeg_preview *>(preview.Get());
            dng_image_writer pw;
            // Quality on Adobe's 0-12 JPEG scale.
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
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

void deptheos_write_dng(const std::string &output_path,
                        const DngImageMetadata &meta,
                        const uint16_t *pixels, size_t /* pixel_count */,
                        int32_t width, int32_t height, int32_t channels,
                        DngWriteKind kind) {
    deptheos_host host;
    // We flush the FILE ourselves: the stream's Flush only reaches stdio's
    // buffer and its destructor closes without a check, so a full disk would
    // go unnoticed. The stream still closes it.
#if qWinOS
    FILE *file = _wfopen(widen_utf8(output_path).c_str(), L"wb");
#else
    FILE *file = std::fopen(output_path.c_str(), "wb");
#endif
    if (!file) ThrowOpenFile();
    dng_file_stream outStream(file);
    write_dng_to_stream(host, outStream, meta, pixels, width, height, channels, kind);
    if (std::fflush(file) != 0 || std::ferror(file)) {
        throw std::runtime_error(std::string("writing the file failed: ") + std::strerror(errno));
    }
}
