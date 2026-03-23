//
//  Untitled.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//
#include "dng_bridge_internal.h"

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

static dng_matrix_3by3 to_dng_matrix(const DngMatrix3x3 &m) {
    if (m.values.size() < 9) return dng_matrix_3by3();
    return dng_matrix_3by3(
        m.values[0], m.values[1], m.values[2],
        m.values[3], m.values[4], m.values[5],
        m.values[6], m.values[7], m.values[8]
    );
}

dng_negative * rebuild_negative_from_metadata(
    dng_host &host, const DngImageMetadata &meta,
    const uint16_t *pixels, int w, int h, int ch) {

    AutoPtr<dng_negative> negative(host.Make_dng_negative());

    negative->SetModelName(meta.camera.uniqueCameraModel.c_str());
    int channels = (ch > 0) ? ch : (meta.geometry.channels > 0 ? meta.geometry.channels : 3);
    negative->SetColorChannels(channels == 1 ? 1 : 3);

    // Rebuild camera profile
    {
        AutoPtr<dng_camera_profile> profile(new dng_camera_profile);

        if (meta.colorScience.colorMatrix1.isValid)
            profile->SetColorMatrix1(to_dng_matrix(meta.colorScience.colorMatrix1));
        if (meta.colorScience.colorMatrix2.isValid)
            profile->SetColorMatrix2(to_dng_matrix(meta.colorScience.colorMatrix2));
        if (meta.colorScience.forwardMatrix1.isValid)
            profile->SetForwardMatrix1(to_dng_matrix(meta.colorScience.forwardMatrix1));
        if (meta.colorScience.forwardMatrix2.isValid)
            profile->SetForwardMatrix2(to_dng_matrix(meta.colorScience.forwardMatrix2));

        profile->SetCalibrationIlluminant1(meta.colorScience.calibrationIlluminant1);
        profile->SetCalibrationIlluminant2(meta.colorScience.calibrationIlluminant2);

        if (!meta.colorScience.profileCalibrationSig.empty())
            profile->SetProfileCalibrationSignature(meta.colorScience.profileCalibrationSig.c_str());
        if (!meta.colorScience.profileName.empty())
            profile->SetName(meta.colorScience.profileName.c_str());

        profile->SetHueSatMapEncoding(meta.luts.hueSatMapEncoding);
        profile->SetLookTableEncoding(meta.luts.lookTableEncoding);

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
            dng_tone_curve tc;
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

    // Calibration matrices — default to identity so SDK doesn't write zeros
    if (meta.colorScience.cameraCalibration1.isValid)
        negative->SetCameraCalibration1(to_dng_matrix(meta.colorScience.cameraCalibration1));
    else
        negative->SetCameraCalibration1(dng_matrix_3by3(1, 0, 0, 0, 1, 0, 0, 0, 1));
    if (meta.colorScience.cameraCalibration2.isValid)
        negative->SetCameraCalibration2(to_dng_matrix(meta.colorScience.cameraCalibration2));
    else
        negative->SetCameraCalibration2(dng_matrix_3by3(1, 0, 0, 0, 1, 0, 0, 0, 1));
    if (!meta.colorScience.cameraCalibrationSig.empty())
        negative->SetCameraCalibrationSignature(meta.colorScience.cameraCalibrationSig.c_str());

    // Analog balance
    if (!meta.colorScience.analogBalance.empty()) {
        uint32 count = (uint32)meta.colorScience.analogBalance.size();
        dng_vector ab(count);
        for (uint32 i = 0; i < count; i++)
            ab[i] = meta.colorScience.analogBalance[i];
        negative->SetAnalogBalance(ab);
    }

    // Camera neutral
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

    // Noise profile
    if (!meta.noiseProfile.scale.empty()) {
        dng_std_vector<dng_noise_function> funcs;
        for (size_t i = 0; i < meta.noiseProfile.scale.size(); i++)
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
    if (!meta.opcodes.opcodeList3.empty()) {
        dng_memory_stream ms(host.Allocator());
        ms.Put(meta.opcodes.opcodeList3.data(), (uint32)meta.opcodes.opcodeList3.size());
        ms.SetReadPosition(0);
        negative->OpcodeList3().Parse(host, ms, (uint32)meta.opcodes.opcodeList3.size(), 0);
    }

    // Image geometry
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
        if (!meta.camera.serialNumber.empty()) exif->fCameraSerialNumber.Set(meta.camera.serialNumber.c_str());
        if (!meta.camera.artist.empty()) exif->fArtist.Set(meta.camera.artist.c_str());
        if (!meta.camera.copyright.empty()) exif->fCopyright.Set(meta.camera.copyright.c_str());
        if (!meta.camera.software.empty()) exif->fSoftware.Set(meta.camera.software.c_str());

        if (meta.exposure.exposureTime.den > 0)
            exif->fExposureTime = dng_urational(meta.exposure.exposureTime.num, meta.exposure.exposureTime.den);
        if (meta.exposure.fnumber.den > 0)
            exif->fFNumber = dng_urational(meta.exposure.fnumber.num, meta.exposure.fnumber.den);
        if (meta.exposure.isoSpeed.size() >= 3) {
            exif->fISOSpeedRatings[0] = meta.exposure.isoSpeed[0];
            exif->fISOSpeedRatings[1] = meta.exposure.isoSpeed[1];
            exif->fISOSpeedRatings[2] = meta.exposure.isoSpeed[2];
        }
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
        if (!meta.lens.serialNumber.empty()) exif->fLensSerialNumber.Set(meta.lens.serialNumber.c_str());
        if (meta.lens.info.size() >= 4) {
            for (int i = 0; i < 4; i++)
                exif->fLensInfo[i] = dng_urational(meta.lens.info[i].num, meta.lens.info[i].den);
        }

        if (!meta.dates.original.empty())
            exif->fDateTimeOriginal.Decode_ISO_8601(meta.dates.original.c_str());
        if (!meta.dates.digitized.empty())
            exif->fDateTimeDigitized.Decode_ISO_8601(meta.dates.digitized.c_str());

        // GPS
        exif->fGPSVersionID = meta.gps.versionId;
        if (!meta.gps.latitudeRef.empty()) exif->fGPSLatitudeRef.Set(meta.gps.latitudeRef.c_str());
        if (meta.gps.latitude.size() >= 3) {
            for (int i = 0; i < 3; i++)
                exif->fGPSLatitude[i] = dng_urational(meta.gps.latitude[i].num, meta.gps.latitude[i].den);
        }
        if (!meta.gps.longitudeRef.empty()) exif->fGPSLongitudeRef.Set(meta.gps.longitudeRef.c_str());
        if (meta.gps.longitude.size() >= 3) {
            for (int i = 0; i < 3; i++)
                exif->fGPSLongitude[i] = dng_urational(meta.gps.longitude[i].num, meta.gps.longitude[i].den);
        }
        exif->fGPSAltitudeRef = meta.gps.altitudeRef;
        exif->fGPSAltitude = dng_urational(meta.gps.altitude.num, meta.gps.altitude.den);
    }

    // Set Stage3 image if pixels provided
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

static std::optional<DngReadResult> read_dng_from_stream(dng_host &host, dng_stream &stream) {
    dng_info info;
    info.Parse(host, stream);
    info.PostParse(host);
    if (!info.IsValidDNG()) return std::nullopt;

    // Detect JXL compression before reading pixels
    bool sourceIsJXL = false;
    if (info.fMainIndex >= 0 && info.fMainIndex < (int32)info.IFDCount()) {
        sourceIsJXL = (info.fIFD[info.fMainIndex]->fCompression == ccJXL);
    }

    AutoPtr<dng_negative> negative(host.Make_dng_negative());
    negative->Parse(host, stream, info);
    negative->PostParse(host, stream, info);
    negative->ReadStage1Image(host, stream, info);
    negative->BuildStage2Image(host);

    const dng_image &correctedImage = *negative->Stage2Image();
    if (correctedImage.PixelType() != ttShort) return std::nullopt;

    // Extract cropped Stage2 pixels
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
    result.pixels.resize(total);

    dng_pixel_buffer pb;
    pb.fArea      = cropArea;
    pb.fPlane     = 0;
    pb.fPlanes    = result.channels;
    pb.fPixelType = ttShort;
    pb.fPixelSize = TagTypeSize(ttShort);
    pb.fRowStep   = result.width * result.channels;
    pb.fColStep   = result.channels;
    pb.fData      = result.pixels.data();
    correctedImage.Get(pb);

    fill_image_metadata(host, *negative, result.metadata);
    result.metadata.geometry.useJXL = sourceIsJXL;

    return result;
}

std::optional<DngReadResult> deptheos_dng_read(const std::string &path) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_file_stream stream(path.c_str());
        return read_dng_from_stream(host, stream);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[read_dng] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[read_dng] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[read_dng] Unknown exception");
        return std::nullopt;
    }
}

std::optional<DngReadResult> deptheos_dng_read_from_bytes(const uint8_t *bytes, size_t byte_count) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_memory_stream stream(host.Allocator());
        stream.Put(bytes, (uint32)byte_count);
        stream.SetReadPosition(0);
        return read_dng_from_stream(host, stream);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[read_dng_from_bytes] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[read_dng_from_bytes] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[read_dng_from_bytes] Unknown exception");
        return std::nullopt;
    }
}

// --- Two-phase API ---

struct DngDecodeState {
    deptheos_host host;
    AutoPtr<dng_negative> negative;
    dng_rect cropArea;
    int channels = 0;
};

static std::optional<DngDecodeHandle> open_dng_from_stream(deptheos_host &host, dng_stream &stream) {
    dng_info info;
    info.Parse(host, stream);
    info.PostParse(host);
    if (!info.IsValidDNG()) return std::nullopt;

    bool sourceIsJXL = false;
    if (info.fMainIndex >= 0 && info.fMainIndex < (int32)info.IFDCount()) {
        sourceIsJXL = (info.fIFD[info.fMainIndex]->fCompression == ccJXL);
    }

    AutoPtr<dng_negative> negative(host.Make_dng_negative());
    negative->Parse(host, stream, info);
    negative->PostParse(host, stream, info);
    negative->ReadStage1Image(host, stream, info);
    negative->BuildStage2Image(host);

    const dng_image &correctedImage = *negative->Stage2Image();
    if (correctedImage.PixelType() != ttShort) return std::nullopt;

    dng_urational cropOriginH = negative->DefaultCropOriginH();
    dng_urational cropOriginV = negative->DefaultCropOriginV();
    dng_urational cropSizeH   = negative->DefaultCropSizeH();
    dng_urational cropSizeV   = negative->DefaultCropSizeV();

    int32 cropX = (int32)(cropOriginH.As_real64() + 0.5);
    int32 cropY = (int32)(cropOriginV.As_real64() + 0.5);
    int32 cropW = (int32)(cropSizeH.As_real64() + 0.5);
    int32 cropH = (int32)(cropSizeV.As_real64() + 0.5);

    dng_rect cropArea(cropY, cropX, cropY + cropH, cropX + cropW);

    DngDecodeHandle handle;
    handle.metadata.width = cropArea.W();
    handle.metadata.height = cropArea.H();
    handle.metadata.channels = correctedImage.Planes();

    fill_image_metadata(host, *negative, handle.metadata.metadata);
    handle.metadata.metadata.geometry.useJXL = sourceIsJXL;

    // Move state to heap so it outlives this function
    auto *state = new DngDecodeState();
    state->negative.Reset(negative.Release());
    state->cropArea = cropArea;
    state->channels = correctedImage.Planes();
    handle._internal = state;

    return handle;
}

std::optional<DngDecodeHandle> deptheos_open_dng(const std::string &path) {
    try {
        // Host is created inside open_dng_from_stream's caller scope,
        // but DngDecodeState needs its own host for the lifetime of the handle.
        // We create a temporary host for parsing, then the state manages its own.
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_file_stream stream(path.c_str());
        return open_dng_from_stream(host, stream);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[open_dng] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[open_dng] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[open_dng] Unknown exception");
        return std::nullopt;
    }
}

std::optional<DngDecodeHandle> deptheos_open_dng_from_bytes(const uint8_t *bytes, size_t byte_count) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        dng_memory_stream stream(host.Allocator());
        stream.Put(bytes, (uint32)byte_count);
        stream.SetReadPosition(0);
        return open_dng_from_stream(host, stream);
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[open_dng_from_bytes] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[open_dng_from_bytes] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[open_dng_from_bytes] Unknown exception");
        return std::nullopt;
    }
}

void deptheos_copy_dng_pixels(DngDecodeHandle &handle, void *dest) {
    auto *state = static_cast<DngDecodeState *>(handle._internal);
    if (!state) return;

    const dng_image &img = *state->negative->Stage2Image();
    int w = handle.metadata.width;
    int h = handle.metadata.height;
    int ch = state->channels;

    dng_pixel_buffer pb;
    pb.fArea      = state->cropArea;
    pb.fPlane     = 0;
    pb.fPlanes    = ch;
    pb.fPixelType = ttShort;
    pb.fPixelSize = TagTypeSize(ttShort);
    pb.fRowStep   = w * ch;
    pb.fColStep   = ch;
    pb.fData      = dest;
    img.Get(pb);
}

void deptheos_copy_dng_pixels_rgba(DngDecodeHandle &handle, void *dest, int32_t rowBytes) {
    auto *state = static_cast<DngDecodeState *>(handle._internal);
    if (!state) return;

    const dng_image &img = *state->negative->Stage2Image();
    int ch = state->channels;

    dng_pixel_buffer pb;
    pb.fArea      = state->cropArea;
    pb.fPlane     = 0;
    pb.fPlanes    = ch;
    pb.fPixelType = ttShort;
    pb.fPixelSize = TagTypeSize(ttShort);
    pb.fRowStep   = rowBytes / static_cast<int>(sizeof(uint16_t));
    pb.fColStep   = 4;
    pb.fData      = dest;
    img.Get(pb);
}

void deptheos_close_dng(DngDecodeHandle &handle) {
    if (handle._internal) {
        delete static_cast<DngDecodeState *>(handle._internal);
        handle._internal = nullptr;
    }
}
