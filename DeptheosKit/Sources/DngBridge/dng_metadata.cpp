//
//  dng_metadata.cpp
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//

#include "dng_bridge_internal.h"

// ---------------------------------------------------------------------------
// Helpers for metadata extraction
// ---------------------------------------------------------------------------

static DngMatrix3x3 extract_matrix(const dng_matrix &m) {
    DngMatrix3x3 result;
    if (m.NotEmpty() && m.Rows() >= 3 && m.Cols() >= 3) {
        result.values.resize(9);
        for (int r = 0; r < 3; r++)
            for (int c = 0; c < 3; c++)
                result.values[r * 3 + c] = m[r][c];
        result.isValid = true;
    }
    return result;
}

static std::string safe_string(const char *src) {
    return src ? std::string(src) : std::string();
}

static std::vector<float> extract_hue_sat_floats(const dng_hue_sat_map &map) {
    uint32 count = map.DeltasCount();
    if (count == 0) return {};
    std::vector<float> buf(count * 3);
    const dng_hue_sat_map::HSBModify *deltas = map.GetConstDeltas();
    for (uint32 i = 0; i < count; i++) {
        buf[i * 3 + 0] = deltas[i].fHueShift;
        buf[i * 3 + 1] = deltas[i].fSatScale;
        buf[i * 3 + 2] = deltas[i].fValScale;
    }
    return buf;
}

static DngHueSatDims extract_hue_sat_dims(const dng_hue_sat_map &map) {
    DngHueSatDims dims;
    map.GetDivisions(dims.hueDivisions, dims.satDivisions, dims.valDivisions);
    return dims;
}

// ---------------------------------------------------------------------------
// Render params — precomputed color science via dng_color_spec
// ---------------------------------------------------------------------------

void fill_render_params(dng_negative &negative, DngRenderParams &out) {
    out = DngRenderParams();

    // Default tone curve to identity
    out.toneCurve.resize(257);
    for (int i = 0; i < 257; i++)
        out.toneCurve[i] = (float)i / 256.0f;

    out.blackLevel = negative.Stage3BlackLevel();
    out.shadowScale = negative.ShadowScaleR().IsValid()
        ? negative.ShadowScale() : 1.0;

    // Baseline exposure (accounting for Stage3Gain like the SDK's render task)
    if (negative.BaselineExposureR().IsValid()) {
        const dng_camera_profile_id profileID;
        out.baselineExposure = negative.TotalBaselineExposure(profileID)
            - (log(negative.Stage3Gain()) / log(2.0));
    }

    // Color science via dng_color_spec
    auto set_identity = [&]() {
        out.cameraToProPhoto = {1,0,0, 0,1,0, 0,0,1};
        out.cameraWhite = {1,1,1};
        out.wbBakedIn = true;
    };

    if (negative.IsMonochrome() || negative.ProfileCount() == 0) {
        set_identity();
        return;
    }

    const dng_camera_profile_id profileID;
    dng_camera_profile profile;
    if (!negative.GetProfileByID(profileID, profile)) {
        set_identity();
        return;
    }

    // Build color spec and set white point (triggers all interpolation)
    AutoPtr<dng_color_spec> spec(negative.MakeColorSpec(profileID));

    if (negative.HasCameraNeutral())
        spec->SetWhiteXY(spec->NeutralToXY(negative.CameraNeutral()));
    else if (negative.HasCameraWhiteXY())
        spec->SetWhiteXY(negative.CameraWhiteXY());
    else
        spec->SetWhiteXY(D55_xy_coord());

    // Camera → ProPhoto RGB
    dng_matrix cameraToRGB = dng_space_ProPhoto::Get().MatrixFromPCS()
                           * spec->CameraToPCS();

    out.cameraToProPhoto.resize(9);
    for (int r = 0; r < 3; r++)
        for (int c = 0; c < 3; c++)
            out.cameraToProPhoto[r * 3 + c] = (float)cameraToRGB[r][c];

    // Camera white
    const dng_vector &cw = spec->CameraWhite();
    out.cameraWhite.resize(3);
    for (int i = 0; i < 3 && i < (int)cw.Count(); i++)
        out.cameraWhite[i] = (float)cw[i];

    // Determine if WB is baked in (ForwardMatrix path)
    out.wbBakedIn = profile.ForwardMatrix1().NotEmpty()
                 || profile.ForwardMatrix2().NotEmpty();

    // HueSatMap — interpolated for this white point
    AutoPtr<dng_hue_sat_map> hsmPtr(profile.HueSatMapForWhite(spec->WhiteXY()));
    if (hsmPtr.Get() && hsmPtr->DeltasCount() > 0) {
        out.hueSatMapDims = extract_hue_sat_dims(*hsmPtr);
        out.hueSatMapData = extract_hue_sat_floats(*hsmPtr);
    }

    // LookTable
    if (profile.HasLookTable()) {
        const dng_hue_sat_map &lt = profile.LookTable();
        out.lookTableDims = extract_hue_sat_dims(lt);
        out.lookTableData = extract_hue_sat_floats(lt);
    }

    // Tone curve: sample at 257 points (GPU pipeline upsamples to 4097 via precomposeToneCurve)
    {
        const dng_1d_function *toneCurve = nullptr;
        AutoPtr<dng_spline_solver> profileCurve;

        if (profile.ToneCurve().IsValid()) {
            profileCurve.Reset(new dng_spline_solver);
            profile.ToneCurve().Solve(*profileCurve.Get());
            toneCurve = profileCurve.Get();
        }

        if (!toneCurve)
            toneCurve = &dng_tone_curve_acr3_default::Get();

        for (int i = 0; i < 257; i++) {
            real64 x = (real64)i / 256.0;
            out.toneCurve[i] = (float)toneCurve->Evaluate(x);
        }
    }
}

std::optional<DngRenderParams> deptheos_compute_render_params(const DngImageMetadata &meta) {
    try {
        deptheos_host host;
        host.SetPreferredSize(0); host.SetMinimumSize(0); host.SetMaximumSize(0);
        AutoPtr<dng_negative> negative(
            rebuild_negative_from_metadata(host, meta, nullptr, 0, 0, 0));
        DngRenderParams out;
        fill_render_params(*negative, out);
        return out;
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[compute_render_params] DNG Exception: %d", (int)e.ErrorCode());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[compute_render_params] Standard exception: %s", e.what());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[compute_render_params] Unknown exception");
        return std::nullopt;
    }
}

// ---------------------------------------------------------------------------
// Full image metadata for DNG write-back
// ---------------------------------------------------------------------------

static void extract_profile_luts(const dng_camera_profile &profile, DngLUTData &luts) {
    luts.hueSatMapEncoding = profile.HueSatMapEncoding();
    luts.lookTableEncoding = profile.LookTableEncoding();

    if (profile.HasHueSatDeltas()) {
        luts.hueSatMapDims1 = extract_hue_sat_dims(profile.HueSatDeltas1());
        luts.hueSatMapData1 = extract_hue_sat_floats(profile.HueSatDeltas1());
    }
    if (profile.HasColorMatrix2()) {
        const dng_hue_sat_map &map2 = profile.HueSatDeltas2();
        if (map2.DeltasCount() > 0) {
            luts.hueSatMapDims2 = extract_hue_sat_dims(map2);
            luts.hueSatMapData2 = extract_hue_sat_floats(map2);
        }
    }
    if (profile.HasLookTable()) {
        luts.lookTableDims = extract_hue_sat_dims(profile.LookTable());
        luts.lookTableData = extract_hue_sat_floats(profile.LookTable());
    }

    // Tone curve (XY knot pairs)
    if (profile.ToneCurve().IsValid()) {
        const auto &coords = profile.ToneCurve().fCoord;
        luts.profileToneCurve.resize(coords.size() * 2);
        for (size_t i = 0; i < coords.size(); i++) {
            luts.profileToneCurve[i * 2]     = (float)coords[i].h;  // x (input)
            luts.profileToneCurve[i * 2 + 1] = (float)coords[i].v;  // y (output)
        }
    }
}

void fill_image_metadata(dng_host &host, dng_negative &negative,
                         DngImageMetadata &out) {
    out = DngImageMetadata();

    // Geometry
    out.geometry.orientation = negative.BaseOrientation().GetTIFF();
    out.geometry.channels = (int32_t)negative.ColorChannels();

    // Camera neutral / analog balance
    if (negative.HasCameraNeutral()) {
        const dng_vector &cn = negative.CameraNeutral();
        uint32 count = std::min(cn.Count(), (uint32)4);
        out.colorScience.asShotNeutral.resize(count);
        for (uint32 i = 0; i < count; i++)
            out.colorScience.asShotNeutral[i] = cn[i];
    }
    {
        uint32 cc = negative.ColorChannels();
        if (cc > 0 && cc <= 4) {
            bool hasAB = false;
            std::vector<double> ab(cc);
            for (uint32 i = 0; i < cc; i++) {
                ab[i] = negative.AnalogBalance(i);
                if (ab[i] != 1.0) hasAB = true;
            }
            if (hasAB) out.colorScience.analogBalance = std::move(ab);
        }
    }

    // Calibration matrices
    if (negative.CameraCalibration1().NotEmpty())
        out.colorScience.cameraCalibration1 = extract_matrix(negative.CameraCalibration1());
    if (negative.CameraCalibration2().NotEmpty())
        out.colorScience.cameraCalibration2 = extract_matrix(negative.CameraCalibration2());
    out.colorScience.cameraCalibrationSig = safe_string(
        negative.CameraCalibrationSignature().Get());
    out.camera.uniqueCameraModel = safe_string(negative.ModelName().Get());

    // Exposure parameters
    if (negative.BaselineExposureR().IsValid())
        out.colorScience.baselineExposure = negative.BaselineExposure();
    if (negative.BaselineNoiseR().IsValid())
        out.colorScience.baselineNoise = negative.BaselineNoise();
    if (negative.BaselineSharpnessR().IsValid())
        out.colorScience.baselineSharpness = negative.BaselineSharpness();
    if (negative.ShadowScaleR().IsValid())
        out.colorScience.shadowScale = negative.ShadowScale();
    if (negative.LinearResponseLimitR().IsValid())
        out.colorScience.linearResponseLimit = negative.LinearResponseLimit();

    // Noise profile
    if (negative.HasNoiseProfile()) {
        const dng_noise_profile &np = negative.NoiseProfile();
        uint32 count = std::min(np.NumFunctions(), (uint32)4);
        out.noiseProfile.scale.resize(count);
        out.noiseProfile.offset.resize(count);
        for (uint32 i = 0; i < count; i++) {
            out.noiseProfile.scale[i] = np.NoiseFunction(i).Scale();
            out.noiseProfile.offset[i] = np.NoiseFunction(i).Offset();
        }
    }

    // Camera profile
    if (negative.ProfileCount() > 0) {
        const dng_camera_profile &profile = negative.ProfileByIndex(0);

        out.colorScience.colorMatrix1 = extract_matrix(profile.ColorMatrix1());
        out.colorScience.colorMatrix2 = extract_matrix(profile.ColorMatrix2());
        out.colorScience.forwardMatrix1 = extract_matrix(profile.ForwardMatrix1());
        out.colorScience.forwardMatrix2 = extract_matrix(profile.ForwardMatrix2());
        out.colorScience.calibrationIlluminant1 = profile.CalibrationIlluminant1();
        out.colorScience.calibrationIlluminant2 = profile.CalibrationIlluminant2();
        out.colorScience.profileCalibrationSig = safe_string(
            profile.ProfileCalibrationSignature().Get());
        out.colorScience.profileName = safe_string(profile.Name().Get());

        extract_profile_luts(profile, out.luts);
    }

    // Opcode lists (serialized blobs)
    {
        const dng_opcode_list &list2 = negative.OpcodeList2();
        if (list2.Count() > 0) {
            AutoPtr<dng_memory_block> block(list2.Spool(host));
            if (block.Get()) {
                size_t sz = block->LogicalSize();
                auto *ptr = static_cast<const uint8_t *>(block->Buffer());
                out.opcodes.opcodeList2.assign(ptr, ptr + sz);
            }
        }
        const dng_opcode_list &list3 = negative.OpcodeList3();
        if (list3.Count() > 0) {
            AutoPtr<dng_memory_block> block(list3.Spool(host));
            if (block.Get()) {
                size_t sz = block->LogicalSize();
                auto *ptr = static_cast<const uint8_t *>(block->Buffer());
                out.opcodes.opcodeList3.assign(ptr, ptr + sz);
            }
        }
    }

    // EXIF
    dng_exif *exif = negative.GetExif();
    if (exif) {
        out.camera.make = safe_string(exif->fMake.Get());
        out.camera.model = safe_string(exif->fModel.Get());
        out.camera.serialNumber = safe_string(exif->fCameraSerialNumber.Get());
        out.camera.software = safe_string(exif->fSoftware.Get());
        out.camera.artist = safe_string(exif->fArtist.Get());
        out.camera.copyright = safe_string(exif->fCopyright.Get());

        // Lens
        out.lens.make = safe_string(exif->fLensMake.Get());
        out.lens.model = safe_string(exif->fLensName.Get());
        out.lens.serialNumber = safe_string(exif->fLensSerialNumber.Get());
        out.lens.info.resize(4);
        for (int i = 0; i < 4; i++) {
            out.lens.info[i].num = exif->fLensInfo[i].n;
            out.lens.info[i].den = exif->fLensInfo[i].d;
        }
        out.lens.focalLengthIn35mm = exif->fFocalLengthIn35mmFilm;

        // Exposure
        if (exif->fExposureTime.IsValid()) {
            out.exposure.exposureTime.num = exif->fExposureTime.n;
            out.exposure.exposureTime.den = exif->fExposureTime.d;
        }
        if (exif->fFNumber.IsValid()) {
            out.exposure.fnumber.num = exif->fFNumber.n;
            out.exposure.fnumber.den = exif->fFNumber.d;
        }
        out.exposure.isoSpeed = {
            exif->fISOSpeedRatings[0],
            exif->fISOSpeedRatings[1],
            exif->fISOSpeedRatings[2]
        };
        if (exif->fFocalLength.IsValid()) {
            out.exposure.focalLength.num = exif->fFocalLength.n;
            out.exposure.focalLength.den = exif->fFocalLength.d;
        }
        if (exif->fMaxApertureValue.IsValid()) {
            out.exposure.maxApertureValue.num = exif->fMaxApertureValue.n;
            out.exposure.maxApertureValue.den = exif->fMaxApertureValue.d;
        }
        if (exif->fExposureBiasValue.IsValid()) {
            out.exposure.exposureBias.num = exif->fExposureBiasValue.n;
            out.exposure.exposureBias.den = exif->fExposureBiasValue.d;
        }
        out.exposure.program = exif->fExposureProgram;
        out.exposure.mode = exif->fExposureMode;
        out.exposure.meteringMode = exif->fMeteringMode;
        out.exposure.flash = exif->fFlash;
        out.exposure.whiteBalance = exif->fWhiteBalance;
        out.exposure.sceneCaptureType = exif->fSceneCaptureType;
        out.exposure.focalPlaneXRes.num = exif->fFocalPlaneXResolution.n;
        out.exposure.focalPlaneXRes.den = exif->fFocalPlaneXResolution.d;
        out.exposure.focalPlaneYRes.num = exif->fFocalPlaneYResolution.n;
        out.exposure.focalPlaneYRes.den = exif->fFocalPlaneYResolution.d;
        out.exposure.focalPlaneResUnit = exif->fFocalPlaneResolutionUnit;

        // Dates
        {
            dng_string dto = exif->fDateTimeOriginal.Encode_ISO_8601();
            out.dates.original = safe_string(dto.Get());
        }
        {
            dng_string dtd = exif->fDateTimeDigitized.Encode_ISO_8601();
            out.dates.digitized = safe_string(dtd.Get());
        }

        // GPS
        out.gps.versionId = exif->fGPSVersionID;
        out.gps.latitudeRef = safe_string(exif->fGPSLatitudeRef.Get());
        out.gps.latitude.resize(3);
        for (int i = 0; i < 3; i++) {
            out.gps.latitude[i].num = exif->fGPSLatitude[i].n;
            out.gps.latitude[i].den = exif->fGPSLatitude[i].d;
        }
        out.gps.longitudeRef = safe_string(exif->fGPSLongitudeRef.Get());
        out.gps.longitude.resize(3);
        for (int i = 0; i < 3; i++) {
            out.gps.longitude[i].num = exif->fGPSLongitude[i].n;
            out.gps.longitude[i].den = exif->fGPSLongitude[i].d;
        }
        out.gps.altitudeRef = exif->fGPSAltitudeRef;
        out.gps.altitude.num = exif->fGPSAltitude.n;
        out.gps.altitude.den = exif->fGPSAltitude.d;
    }
}

// ---------------------------------------------------------------------------
// DCP (DNG Camera Profile) loading
// ---------------------------------------------------------------------------

std::optional<DngDcpResult> deptheos_load_dcp(const std::string &dcp_path) {
    try {
        dng_file_stream stream(dcp_path.c_str());
        dng_camera_profile profile;

        if (!profile.ParseExtended(stream)) {
            dng_log(DNG_LOG_WARN, "[load_dcp] ParseExtended failed for %s", dcp_path.c_str());
            return std::nullopt;
        }

        dng_log(DNG_LOG_INFO, "[load_dcp] Loaded profile '%s' from %s",
                profile.Name().Get(), dcp_path.c_str());

        DngDcpResult result;
        auto &cs = result.profile.colorScience;

        // Color matrices
        if (profile.ColorMatrix1().NotEmpty())
            cs.colorMatrix1 = extract_matrix(profile.ColorMatrix1());
        if (profile.ColorMatrix2().NotEmpty())
            cs.colorMatrix2 = extract_matrix(profile.ColorMatrix2());

        // Forward matrices
        if (profile.ForwardMatrix1().NotEmpty())
            cs.forwardMatrix1 = extract_matrix(profile.ForwardMatrix1());
        if (profile.ForwardMatrix2().NotEmpty())
            cs.forwardMatrix2 = extract_matrix(profile.ForwardMatrix2());

        // Illuminants
        cs.calibrationIlluminant1 = profile.CalibrationIlluminant1();
        cs.calibrationIlluminant2 = profile.CalibrationIlluminant2();

        // Profile name and calibration signature
        cs.profileName = safe_string(profile.Name().Get());
        cs.profileCalibrationSig = safe_string(
            profile.ProfileCalibrationSignature().Get());

        // LUTs
        extract_profile_luts(profile, result.profile.luts);

        // Baseline exposure offset
        const dng_srational &beo = profile.BaselineExposureOffset();
        if (beo.d != 0) {
            result.baselineExposureOffset = beo.As_real64();
        }

        return result;
    } catch (const dng_exception &e) {
        dng_log(DNG_LOG_ERROR, "[load_dcp] DNG exception: %d for %s",
                (int)e.ErrorCode(), dcp_path.c_str());
        return std::nullopt;
    } catch (const std::exception &e) {
        dng_log(DNG_LOG_ERROR, "[load_dcp] Standard exception: %s for %s", e.what(), dcp_path.c_str());
        return std::nullopt;
    } catch (...) {
        dng_log(DNG_LOG_ERROR, "[load_dcp] Unknown exception for %s", dcp_path.c_str());
        return std::nullopt;
    }
}
