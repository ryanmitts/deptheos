// Reading DNGs, and rebuilding an SDK negative from DngImageMetadata (the
// first step of rendering, render params and writing).
#include "dng_bridge_internal.h"

#include <chrono>

static void rebuild_hue_sat_map(dng_hue_sat_map &map,
                                 const DngHueSatDims &dims,
                                 const std::vector<float> &data) {
    if (data.empty() || dims.hueDivisions == 0) return;
    map.SetDivisions(dims.hueDivisions, dims.satDivisions, dims.valDivisions);
    uint32 n = map.DeltasCount();
    if (n * 3 != data.size()) return;
    dng_hue_sat_map::HSBModify *deltas = map.GetDeltas();
    for (uint32 i = 0; i < n; i++) {
        deltas[i].fHueShift = data[i * 3 + 0];
        deltas[i].fSatScale = data[i * 3 + 1];
        deltas[i].fValScale = data[i * 3 + 2];
    }
}

static dng_matrix_3by3 to_dng_matrix(const std::array<double, 9> &m) {
    return dng_matrix_3by3(
        m[0], m[1], m[2],
        m[3], m[4], m[5],
        m[6], m[7], m[8]
    );
}

dng_negative * rebuild_negative_from_metadata(
    dng_host &host, const DngImageMetadata &meta,
    const uint16_t *pixels, int w, int h, int ch) {

    AutoPtr<dng_negative> negative(host.Make_dng_negative());

    negative->SetModelName(meta.camera.uniqueCameraModel.c_str());
    int channels = (ch > 0) ? ch : (meta.geometry.channels > 0 ? meta.geometry.channels : 3);
    negative->SetColorChannels(channels == 1 ? 1 : 3);

    {
        AutoPtr<dng_camera_profile> profile(new dng_camera_profile);

        if (meta.colorScience.colorMatrix1)
            profile->SetColorMatrix1(to_dng_matrix(*meta.colorScience.colorMatrix1));
        if (meta.colorScience.colorMatrix2)
            profile->SetColorMatrix2(to_dng_matrix(*meta.colorScience.colorMatrix2));
        if (meta.colorScience.forwardMatrix1)
            profile->SetForwardMatrix1(to_dng_matrix(*meta.colorScience.forwardMatrix1));
        if (meta.colorScience.forwardMatrix2)
            profile->SetForwardMatrix2(to_dng_matrix(*meta.colorScience.forwardMatrix2));

        profile->SetCalibrationIlluminant1(meta.colorScience.calibrationIlluminant1);
        profile->SetCalibrationIlluminant2(meta.colorScience.calibrationIlluminant2);

        if (!meta.colorScience.profileCalibrationSig.empty())
            profile->SetProfileCalibrationSignature(meta.colorScience.profileCalibrationSig.c_str());
        if (!meta.colorScience.profileName.empty())
            profile->SetName(meta.colorScience.profileName.c_str());

        profile->SetHueSatMapEncoding(meta.luts.hueSatMapEncoding);
        profile->SetLookTableEncoding(meta.luts.lookTableEncoding);
        profile->SetDefaultBlackRender(meta.luts.defaultBlackRender);
        if (meta.luts.baselineExposureOffset != 0.0)
            profile->SetBaselineExposureOffset(meta.luts.baselineExposureOffset);

        if (!meta.luts.hueSatMapData1.empty() && meta.luts.hueSatMapDims1.has_value()) {
            dng_hue_sat_map hsm;
            rebuild_hue_sat_map(hsm, *meta.luts.hueSatMapDims1, meta.luts.hueSatMapData1);
            profile->SetHueSatDeltas1(hsm);
        }
        if (!meta.luts.hueSatMapData2.empty() && meta.luts.hueSatMapDims2.has_value()) {
            dng_hue_sat_map hsm;
            rebuild_hue_sat_map(hsm, *meta.luts.hueSatMapDims2, meta.luts.hueSatMapData2);
            profile->SetHueSatDeltas2(hsm);
        }
        if (!meta.luts.lookTableData.empty() && meta.luts.lookTableDims.has_value()) {
            dng_hue_sat_map lt;
            rebuild_hue_sat_map(lt, *meta.luts.lookTableDims, meta.luts.lookTableData);
            profile->SetLookTable(lt);
        }

        // Tone curve (XY knot pairs)
        if (meta.luts.profileToneCurve.size() >= 4) {
            // A new curve holds the identity's two points; the knots replace them.
            dng_tone_curve tc;
            tc.fCoord.clear();
            uint32 knots = (uint32)meta.luts.profileToneCurve.size() / 2;
            for (uint32 i = 0; i < knots; i++) {
                tc.fCoord.push_back(dng_point_real64(
                    meta.luts.profileToneCurve[i * 2 + 1],  // v = y (output)
                    meta.luts.profileToneCurve[i * 2]        // h = x (input)
                ));
            }
            profile->SetToneCurve(tc);
        }

        negative->AddProfile(profile);
    }

    // Identity when absent, so the SDK doesn't write zero matrices.
    if (meta.colorScience.cameraCalibration1)
        negative->SetCameraCalibration1(to_dng_matrix(*meta.colorScience.cameraCalibration1));
    else
        negative->SetCameraCalibration1(dng_matrix_3by3(1, 0, 0, 0, 1, 0, 0, 0, 1));
    if (meta.colorScience.cameraCalibration2)
        negative->SetCameraCalibration2(to_dng_matrix(*meta.colorScience.cameraCalibration2));
    else
        negative->SetCameraCalibration2(dng_matrix_3by3(1, 0, 0, 0, 1, 0, 0, 0, 1));
    if (!meta.colorScience.cameraCalibrationSig.empty())
        negative->SetCameraCalibrationSignature(meta.colorScience.cameraCalibrationSig.c_str());

    if (!meta.colorScience.analogBalance.empty()) {
        uint32 count = (uint32)meta.colorScience.analogBalance.size();
        dng_vector ab(count);
        for (uint32 i = 0; i < count; i++)
            ab[i] = meta.colorScience.analogBalance[i];
        negative->SetAnalogBalance(ab);
    }

    if (!meta.colorScience.asShotNeutral.empty()) {
        uint32 count = (uint32)meta.colorScience.asShotNeutral.size();
        dng_vector cn(count);
        for (uint32 i = 0; i < count; i++)
            cn[i] = meta.colorScience.asShotNeutral[i];
        negative->SetCameraNeutral(cn);
    }

    // Exposure parameters
    if (meta.colorScience.baselineExposure.has_value())
        negative->SetBaselineExposure(*meta.colorScience.baselineExposure);
    if (meta.colorScience.baselineNoise.has_value())
        negative->SetBaselineNoise(*meta.colorScience.baselineNoise);
    if (meta.colorScience.baselineSharpness.has_value())
        negative->SetBaselineSharpness(*meta.colorScience.baselineSharpness);
    if (meta.colorScience.linearResponseLimit.has_value())
        negative->SetLinearResponseLimit(*meta.colorScience.linearResponseLimit);
    if (meta.colorScience.shadowScale.has_value())
        negative->SetShadowScale(dng_urational((uint32)(*meta.colorScience.shadowScale * 100 + 0.5), 100));

    // Noise profile: one function per (scale, offset) pair both vectors cover.
    const size_t noiseCount = std::min(meta.noiseProfile.scale.size(),
                                       meta.noiseProfile.offset.size());
    if (noiseCount > 0) {
        dng_std_vector<dng_noise_function> funcs;
        for (size_t i = 0; i < noiseCount; i++)
            funcs.push_back(dng_noise_function(meta.noiseProfile.scale[i], meta.noiseProfile.offset[i]));
        negative->SetNoiseProfile(dng_noise_profile(funcs));
    }

    // Opcode lists
    if (!meta.opcodes.opcodeList2.empty()) {
        dng_memory_stream ms(host.Allocator());
        ms.Put(meta.opcodes.opcodeList2.data(), (uint32)meta.opcodes.opcodeList2.size());
        ms.SetReadPosition(0);
        negative->OpcodeList2().Parse(host, ms, (uint32)meta.opcodes.opcodeList2.size(), 0);
    }
    // List 3 only where the pixels are framed as its stage 3 was.
    const bool list3Applies = w > 0 && h > 0 && w == meta.opcodes.list3Width &&
                              h == meta.opcodes.list3Height;
    if (!meta.opcodes.opcodeList3.empty() && !list3Applies) {
        dng_log(DNG_LOG_DEBUG,
                "[rebuild_negative] opcode list 3 left out: it applies to %dx%d, not %dx%d",
                meta.opcodes.list3Width, meta.opcodes.list3Height, w, h);
    }
    if (!meta.opcodes.opcodeList3.empty() && list3Applies) {
        dng_memory_stream ms(host.Allocator());
        ms.Put(meta.opcodes.opcodeList3.data(), (uint32)meta.opcodes.opcodeList3.size());
        ms.SetReadPosition(0);
        negative->OpcodeList3().Parse(host, ms, (uint32)meta.opcodes.opcodeList3.size(), 0);
    }

    // The pixels are already linear, black at 0 and white at full scale.
    negative->SetBlackLevel(0.0);
    negative->SetWhiteLevel(65535);

    if (w > 0 && h > 0) {
        dng_rect bounds(h, w);
        negative->SetDefaultCropSize(w, h);
        negative->SetDefaultCropOrigin(0, 0);
        negative->SetDefaultOriginalSizes();
        negative->SetActiveArea(bounds);
    }

    negative->SetBaseOrientation(dng_orientation::Normal());
    if (meta.geometry.orientation >= 1 && meta.geometry.orientation <= 8)
        negative->SetBaseOrientation(dng_orientation::TIFFtoDNG(meta.geometry.orientation));

    // EXIF
    dng_exif *exif = negative->GetExif();
    if (exif) {
        exif->fMake.Set(meta.camera.make.c_str());
        exif->fModel.Set(meta.camera.model.c_str());
        if (!meta.camera.artist.empty()) exif->fArtist.Set(meta.camera.artist.c_str());
        if (!meta.camera.copyright.empty()) exif->fCopyright.Set(meta.camera.copyright.c_str());
        if (!meta.camera.software.empty()) exif->fSoftware.Set(meta.camera.software.c_str());

        if (meta.exposure.exposureTime.den > 0)
            exif->fExposureTime = dng_urational(meta.exposure.exposureTime.num, meta.exposure.exposureTime.den);
        if (meta.exposure.fnumber.den > 0)
            exif->fFNumber = dng_urational(meta.exposure.fnumber.num, meta.exposure.fnumber.den);
        for (int i = 0; i < 3; i++)
            exif->fISOSpeedRatings[i] = meta.exposure.isoSpeed[i];
        exif->fExposureProgram = meta.exposure.program;
        if (meta.exposure.exposureBias.den != 0)
            exif->fExposureBiasValue = dng_srational(meta.exposure.exposureBias.num, meta.exposure.exposureBias.den);
        exif->fMeteringMode = meta.exposure.meteringMode;
        exif->fFlash = meta.exposure.flash;
        if (meta.exposure.focalLength.den > 0)
            exif->fFocalLength = dng_urational(meta.exposure.focalLength.num, meta.exposure.focalLength.den);
        exif->fFocalLengthIn35mmFilm = meta.lens.focalLengthIn35mm;
        if (meta.exposure.maxApertureValue.den > 0)
            exif->fMaxApertureValue = dng_urational(meta.exposure.maxApertureValue.num, meta.exposure.maxApertureValue.den);
        exif->fExposureMode = meta.exposure.mode;
        exif->fWhiteBalance = meta.exposure.whiteBalance;
        exif->fSceneCaptureType = meta.exposure.sceneCaptureType;
        if (meta.exposure.focalPlaneXRes.den > 0)
            exif->fFocalPlaneXResolution = dng_urational(meta.exposure.focalPlaneXRes.num, meta.exposure.focalPlaneXRes.den);
        if (meta.exposure.focalPlaneYRes.den > 0)
            exif->fFocalPlaneYResolution = dng_urational(meta.exposure.focalPlaneYRes.num, meta.exposure.focalPlaneYRes.den);
        exif->fFocalPlaneResolutionUnit = meta.exposure.focalPlaneResUnit;
        if (!meta.lens.model.empty()) exif->fLensName.Set(meta.lens.model.c_str());
        if (!meta.lens.make.empty()) exif->fLensMake.Set(meta.lens.make.c_str());
        for (int i = 0; i < 4; i++)
            exif->fLensInfo[i] = dng_urational(meta.lens.info[i].num, meta.lens.info[i].den);

        if (!meta.dates.original.empty())
            exif->fDateTimeOriginal.Decode_ISO_8601(meta.dates.original.c_str());
        if (!meta.dates.digitized.empty())
            exif->fDateTimeDigitized.Decode_ISO_8601(meta.dates.digitized.c_str());
    }

    // The pixels, when given, become stage 3.
    if (pixels && w > 0 && h > 0) {
        dng_rect bounds(h, w);
        uint32 planes = (channels == 1) ? 1 : 3;
        AutoPtr<dng_simple_image> newImage(
            new dng_simple_image(bounds, planes, ttShort, host.Allocator()));

        dng_pixel_buffer buffer;
        buffer.fArea      = bounds;
        buffer.fPlane     = 0;
        buffer.fPlanes    = planes;
        buffer.fPixelType = ttShort;
        buffer.fPixelSize = sizeof(uint16_t);
        buffer.fRowStep   = w * planes;
        buffer.fColStep   = planes;
        buffer.fData      = (void *)pixels;
        newImage->Put(buffer);

        AutoPtr<dng_image> imagePtr(newImage.Release());
        negative->SetStage3Image(imagePtr);
    }

    return negative.Release();
}

// Keeps opcode list 3 in `meta` only when the image a read hands out is the
// whole stage 3 (the default crop is the active area, upright as stored, a
// mosaic demosaiced at full size), or the OriginalDefaultCropSize of a color
// reference (dng_write.cpp). Otherwise no reader should warp or shade a
// picture framed differently from the one it was made for.
static void frame_opcode_list3(const dng_info &info, dng_negative &negative,
                               DngImageMetadata &meta) {
    meta.opcodes.list3Width = 0;
    meta.opcodes.list3Height = 0;
    if (meta.opcodes.opcodeList3.empty()) return;
    const dng_rect active = info.fIFD[info.fMainIndex]->fActiveArea;
    const bool whole =
        negative.DefaultCropOriginH().As_real64() == 0.0 &&
        negative.DefaultCropOriginV().As_real64() == 0.0 &&
        negative.DefaultCropSizeH().As_real64() == (real64)active.W() &&
        negative.DefaultCropSizeV().As_real64() == (real64)active.H() &&
        negative.BaseOrientation() == dng_orientation::Normal() &&
        (!negative.GetMosaicInfo() ||
         (negative.RawToFullScaleH() == 1.0 && negative.RawToFullScaleV() == 1.0));
    if (!whole) {
        dng_log(DNG_LOG_DEBUG,
                "[read_dng] opcode list 3 left out: the image is cropped or turned from its stage 3");
        meta.opcodes.opcodeList3.clear();
        return;
    }
    const bool original = negative.OriginalDefaultCropSizeH().As_real64() >= 1.0 &&
                          negative.OriginalDefaultCropSizeV().As_real64() >= 1.0;
    meta.opcodes.list3Width = original
        ? (int32_t)(negative.OriginalDefaultCropSizeH().As_real64() + 0.5) : (int32_t)active.W();
    meta.opcodes.list3Height = original
        ? (int32_t)(negative.OriginalDefaultCropSizeV().As_real64() + 0.5) : (int32_t)active.H();
}

// Every image the bridge hands out is upright, like LibRaw's: the
// orientation is applied to the pixels and the metadata says Normal, so a
// DNG written from it isn't rotated twice. On entry `width` and `height`
// are the stored size.
static void report_upright(DngReadResult &result, const dng_orientation &orientation) {
    if (orientation.FlipD()) {
        std::swap(result.width, result.height);
    }
    result.metadata.geometry.orientation = 1;
}

// Turns `pixels` (`result`'s stored size) upright.
static void rotate_pixels(dng_host &host, const DngReadResult &result, uint16_t *pixels,
                          const dng_orientation &orientation) {
    if (orientation == dng_orientation::Normal()) {
        return;
    }
    const uint32 planes = (uint32)result.channels;
    dng_simple_image image(dng_rect(result.height, result.width), planes, ttShort,
                           host.Allocator());
    dng_pixel_buffer stored;
    stored.fArea      = image.Bounds();
    stored.fPlane     = 0;
    stored.fPlanes    = planes;
    stored.fPixelType = ttShort;
    stored.fPixelSize = TagTypeSize(ttShort);
    stored.fRowStep   = result.width * planes;
    stored.fColStep   = planes;
    stored.fData      = pixels;
    image.Put(stored);

    // Rotate only re-points the image's strides; Get copies it out upright.
    image.Rotate(orientation);
    const dng_rect upright = image.Bounds();
    dng_pixel_buffer out(stored);
    out.fArea    = upright;
    out.fRowStep = upright.W() * planes;
    out.fData    = pixels;
    image.Get(out);
}

// Copies `area` of `image` into `out`, rows of area.W() × `planes` samples,
// in row bands on the host's threads (a 45 MP frame: 44 ms on one thread,
// 13 in bands).
static void copy_out(deptheos_host &host, const dng_image &image, const dng_rect &area,
                     uint32 planes, uint16_t *out) {
    const size_t rowSamples = static_cast<size_t>(area.W()) * planes;
    const size_t bytes = rowSamples * area.H() * sizeof(uint16_t);
    // Get leaves what lies outside the image untouched, and `out` may be
    // uninitialized.
    if ((area & image.Bounds()) != area) {
        std::memset(out, 0, bytes);
    }
    auto get = [&](int32 top, int32 bottom) {
        dng_pixel_buffer pb;
        pb.fArea      = dng_rect(top, area.l, bottom, area.r);
        pb.fPlane     = 0;
        pb.fPlanes    = planes;
        pb.fPixelType = ttShort;
        pb.fPixelSize = TagTypeSize(ttShort);
        pb.fRowStep   = static_cast<int32>(rowSamples);
        pb.fColStep   = planes;
        pb.fData      = out + static_cast<size_t>(top - area.t) * rowSamples;
        image.Get(pb);
    };
    for_each_band(host, area.t, area.b, bytes, get);
}

struct ReadRequest {
    // Where the pixels go (`result.pixels` stays empty); none for a
    // metadata read, which skips stage 1, stage 2 and every pixel buffer
    // (a full 44 MP read costs ≈ 264 MB and most of a second).
    const DngPixelAlloc *alloc = nullptr;
    // Windows only: read an uncompressed image around the OS file cache.
    bool bypassCache = false;
    // The most pixels the file's image, or its default crop, may have.
    uint64_t maxPixels = 0;

    static ReadRequest metadata() { return {}; }
    static ReadRequest pixels(const DngPixelAlloc &alloc, bool bypassCache, uint64_t maxPixels) {
        ReadRequest request;
        request.alloc       = &alloc;
        request.bypassCache = bypassCache;
        request.maxPixels   = maxPixels;
        return request;
    }
    bool metadataOnly() const { return alloc == nullptr; }
};

// `path`, the file `stream` reads, lets an uncompressed stage 1 be read on
// several threads (dng_fast_read.cpp). Stage times are logged at debug.
static DngReadResult read_dng_from_stream(deptheos_host &host, dng_stream &stream,
                                          const std::string &path, const ReadRequest &request) {
    auto mark = std::chrono::steady_clock::now();
    auto lap = [&mark] {
        const auto now = std::chrono::steady_clock::now();
        const double ms = std::chrono::duration<double, std::milli>(now - mark).count();
        mark = now;
        return ms;
    };

    dng_info info;
    info.Parse(host, stream);
    info.PostParse(host);
    // Refused before the SDK allocates: stage 1, stage 2 and the copy out
    // each take a frame's worth.
    if (!request.metadataOnly() && info.fMainIndex >= 0 &&
        info.fMainIndex < (int32)info.IFDCount()) {
        const dng_ifd &ifd = *info.fIFD[info.fMainIndex];
        const uint64_t stored = (uint64_t)ifd.fImageWidth * ifd.fImageLength;
        const uint64_t crop = (uint64_t)(ifd.fDefaultCropSizeH.As_real64() + 0.5) *
                              (uint64_t)(ifd.fDefaultCropSizeV.As_real64() + 0.5);
        if (stored > request.maxPixels || crop > request.maxPixels) {
            dng_log(DNG_LOG_WARN, "[read_dng] %ux%u is more than %llu pixels: %s",
                    (unsigned)ifd.fImageWidth, (unsigned)ifd.fImageLength,
                    (unsigned long long)request.maxPixels, path.c_str());
            throw dng_exception(DT_ERROR_TOO_LARGE);
        }
    }
    if (!info.IsValidDNG()) {
        dng_log(DNG_LOG_WARN, "[read_dng] not a valid DNG: %s", path.c_str());
        ThrowBadFormat();
    }

    // Reported as `geometry.useJXL`, so a write compresses with JPEG XL too.
    bool sourceIsJXL = false;
    if (info.fMainIndex >= 0 && info.fMainIndex < (int32)info.IFDCount()) {
        sourceIsJXL = (info.fIFD[info.fMainIndex]->fCompression == ccJXL);
    }

    AutoPtr<dng_negative> negative(host.Make_dng_negative());
    negative->Parse(host, stream, info);
    negative->PostParse(host, stream, info);

    // A CFA negative's stage 2 is the mosaic, so it returns metadata with no
    // pixels (channels = 0) and LibRaw demosaics (ADR-131). A metadata-only
    // read takes the same exit.
    const bool mosaic = negative->GetMosaicInfo() != nullptr;
    if (request.metadataOnly() || mosaic) {
        // The opcode lists are read with the image, which this read skips.
        negative->ReadOpcodeLists(host, stream, info);
        DngReadResult result;
        result.width  = (int32)(negative->DefaultCropSizeH().As_real64() + 0.5);
        result.height = (int32)(negative->DefaultCropSizeV().As_real64() + 0.5);
        fill_image_metadata(host, *negative, result.metadata);
        frame_opcode_list3(info, *negative, result.metadata);
        result.metadata.geometry.useJXL = sourceIsJXL;
        report_upright(result, negative->BaseOrientation());
        // Matches the full path, where BuildStage2Image consumes list 2.
        result.metadata.opcodes.opcodeList2.clear();
        return result;
    }

    const double parseMs = lap();
    const stage1_read stage1 =
        read_stage1_fast(host, stream, info, *negative, path, request.bypassCache);
    if (stage1 == stage1_read::not_read) negative->ReadStage1Image(host, stream, info);
    // Which path stage 1 took, for the log.
    const char *stage1Path = "";
    switch (stage1) {
    case stage1_read::not_read: break;
    case stage1_read::uncompressed: stage1Path = " (parallel)"; break;
    case stage1_read::lossless_jpeg: stage1Path = " (fast decode)"; break;
    case stage1_read::lossless_jpeg_partly: stage1Path = " (fast decode, partly)"; break;
    }
    const double stage1Ms = lap();
    // The black-level tables can wait for the copy out.
    auto *fast = dynamic_cast<deptheos_negative *>(negative.Get());
    if (fast && !dng_reference_reads()) fast->fDeferLinearization = true;
    negative->BuildStage2Image(host);
    const bool deferred = fast && fast->LinearizationDeferred();
    dng_record_read_path(stage1, deferred);
    const double stage2Ms = lap();

    const dng_image &correctedImage = *negative->Stage2Image();
    if (correctedImage.PixelType() != ttShort) {
        if (correctedImage.PixelType() == ttFloat) {
            dng_log(DNG_LOG_WARN, "[read_dng] floating-point DNG not supported: %s", path.c_str());
        } else {
            dng_log(DNG_LOG_WARN, "[read_dng] DNG pixel type %u not supported: %s",
                    (unsigned)correctedImage.PixelType(), path.c_str());
        }
        ThrowNotYetImplemented();
    }

    // Copy out the default crop of Stage 2.
    dng_urational cropOriginH = negative->DefaultCropOriginH();
    dng_urational cropOriginV = negative->DefaultCropOriginV();
    dng_urational cropSizeH   = negative->DefaultCropSizeH();
    dng_urational cropSizeV   = negative->DefaultCropSizeV();

    int32 cropX = (int32)(cropOriginH.As_real64() + 0.5);
    int32 cropY = (int32)(cropOriginV.As_real64() + 0.5);
    int32 cropW = (int32)(cropSizeH.As_real64() + 0.5);
    int32 cropH = (int32)(cropSizeV.As_real64() + 0.5);

    dng_rect cropArea(cropY, cropX, cropY + cropH, cropX + cropW);

    DngReadResult result;
    result.width    = cropArea.W();
    result.height   = cropArea.H();
    result.channels = correctedImage.Planes();

    const size_t total = static_cast<size_t>(result.width) * result.height * result.channels;
    uint16_t *pixels = (*request.alloc)(total);
    if (!pixels) {
        dng_log(DNG_LOG_ERROR, "[read_dng] no buffer for %zu samples", total);
        ThrowMemoryFull();
    }
    lap();
    if (deferred) {
        copy_out_linearized(host, *fast, cropArea, (uint32)result.channels, pixels);
    } else {
        copy_out(host, correctedImage, cropArea, (uint32)result.channels, pixels);
    }
    const double copyMs = lap();

    fill_image_metadata(host, *negative, result.metadata);
    frame_opcode_list3(info, *negative, result.metadata);
    result.metadata.geometry.useJXL = sourceIsJXL;
    const double metadataMs = lap();
    // Freed before the rotation allocates, so a turned frame isn't held
    // three times over.
    const dng_orientation orientation = negative->BaseOrientation();
    negative.Reset();
    rotate_pixels(host, result, pixels, orientation);
    const double rotateMs = lap();
    report_upright(result, orientation);

    dng_log(DNG_LOG_DEBUG,
            "[read_dng] %dx%dx%d: parse %.0f ms, stage 1 %.0f ms%s, stage 2 %.0f ms, copy %.0f ms%s, "
            "metadata %.0f ms, rotate %.0f ms",
            result.width, result.height, result.channels, parseMs, stage1Ms,
            stage1Path, stage2Ms,
            copyMs, deferred ? " (with black level)" : "", metadataMs, rotateMs);
    return result;
}

DngReadResult deptheos_dng_read_into(const std::string &path, bool bypassCache,
                                     uint64_t maxPixels, const DngPixelAlloc &alloc,
                                     uint32_t threads, const uint8_t *cancel) {
    deptheos_host host(threads, cancel);
    utf8_file_stream stream(path);
    return read_dng_from_stream(host, stream, path,
                                ReadRequest::pixels(alloc, bypassCache, maxPixels));
}

DngReadResult deptheos_dng_read_metadata(const std::string &path) {
    deptheos_host host;
    utf8_file_stream stream(path);
    return read_dng_from_stream(host, stream, path, ReadRequest::metadata());
}
