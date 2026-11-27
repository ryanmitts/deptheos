// Metadata in and out of the SDK: DngImageMetadata from a parsed negative,
// GPU render parameters and white points from dng_color_spec, temperatures
// from dng_temperature, and DCP profiles applied.

#include "dng_bridge_internal.h"
#include "dng_c_api.h"
#include "dng_temperature.h"

// ---------------------------------------------------------------------------
// Helpers for metadata extraction
// ---------------------------------------------------------------------------

static DngMatrix3x3 extract_matrix(const dng_matrix &m) {
    if (!(m.NotEmpty() && m.Rows() >= 3 && m.Cols() >= 3)) return std::nullopt;
    std::array<double, 9> result;
    for (int r = 0; r < 3; r++)
        for (int c = 0; c < 3; c++)
            result[r * 3 + c] = m[r][c];
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

// Divisions, and deltas in a new[]'d array (null when empty) that
// dt_dng_render_params_free deletes.
static void put_hue_sat(const dng_hue_sat_map &map, uint32_t dims[3], float *&data,
                        int64_t &count) {
    map.GetDivisions(dims[0], dims[1], dims[2]);
    const std::vector<float> floats = extract_hue_sat_floats(map);
    count = static_cast<int64_t>(floats.size());
    if (floats.empty()) return;
    data = new float[floats.size()];
    std::copy(floats.begin(), floats.end(), data);
}

// The color spec of a negative's default profile, or none for a monochrome
// negative or one without a usable profile (those render through identity).
static std::unique_ptr<dng_color_spec> make_color_spec(dng_negative &negative,
                                                       dng_camera_profile &profile) {
    const dng_camera_profile_id profileID;
    if (negative.IsMonochrome() || negative.ProfileCount() == 0 ||
        !negative.GetProfileByID(profileID, profile)) {
        return nullptr;
    }
    return std::unique_ptr<dng_color_spec>(negative.MakeColorSpec(profileID));
}

// Sets `white`, or the white the SDK's render takes as shot: the camera
// neutral's, else the camera white, else D55 (as dng_render_task::Start).
// Setting it interpolates a dual-illuminant profile's matrices for it.
static void set_white(dng_color_spec &spec, const dng_negative &negative,
                      const std::optional<DngWhiteXY> &white) {
    if (white)
        spec.SetWhiteXY(dng_xy_coord(white->x, white->y));
    else if (negative.HasCameraNeutral())
        spec.SetWhiteXY(spec.NeutralToXY(negative.CameraNeutral()));
    else if (negative.HasCameraWhiteXY())
        spec.SetWhiteXY(negative.CameraWhiteXY());
    else
        spec.SetWhiteXY(D55_xy_coord());
}

// `out` starts zeroed.
static void fill_render_params(dng_negative &negative, const std::optional<DngWhiteXY> &white,
                               DtDngRenderParams &out) {
    for (int i = 0; i < 257; i++)
        out.tone_curve[i] = (float)i / 256.0f;

    out.black_level = negative.Stage3BlackLevel();
    // As dng_render_task::Start: shadows x ShadowScale x Stage3Gain (the
    // profile's DefaultBlackRender is applied below).
    out.shadow_scale = negative.ShadowScale() * negative.Stage3Gain();

    // Baseline exposure as the SDK's render task takes it: plus the
    // profile's offset, less Stage3Gain.
    {
        const dng_camera_profile_id profileID;
        out.baseline_exposure = negative.TotalBaselineExposure(profileID)
            - (log(negative.Stage3Gain()) / log(2.0));
    }

    auto set_identity = [&]() {
        for (int i = 0; i < 9; i++) out.camera_to_prophoto[i] = (i % 4 == 0) ? 1.0f : 0.0f;
        for (int i = 0; i < 3; i++) out.camera_white[i] = 1.0f;
    };

    dng_camera_profile profile;
    std::unique_ptr<dng_color_spec> spec = make_color_spec(negative, profile);
    if (!spec) {
        set_identity();
        return;
    }
    set_white(*spec, negative, white);

    // Camera → ProPhoto RGB
    dng_matrix cameraToRGB = dng_space_ProPhoto::Get().MatrixFromPCS()
                           * spec->CameraToPCS();
    for (int r = 0; r < 3; r++)
        for (int c = 0; c < 3; c++)
            out.camera_to_prophoto[r * 3 + c] = (float)cameraToRGB[r][c];

    const dng_vector &cw = spec->CameraWhite();
    for (int i = 0; i < 3 && i < (int)cw.Count(); i++)
        out.camera_white[i] = (float)cw[i];

    // As dng_render's constructor: a profile can turn the shadow ramp off.
    if (profile.DefaultBlackRender() == defaultBlackRender_None)
        out.shadow_scale = 0.0;

    // HueSatMap — interpolated for this white point
    AutoPtr<dng_hue_sat_map> hsmPtr(profile.HueSatMapForWhite(spec->WhiteXY()));
    if (hsmPtr.Get() && hsmPtr->DeltasCount() > 0) {
        put_hue_sat(*hsmPtr, out.hue_sat_dims, out.hue_sat_data, out.hue_sat_count);
        out.hue_sat_encoding = profile.HueSatMapEncoding();
    }

    // LookTable
    if (profile.HasLookTable()) {
        put_hue_sat(profile.LookTable(), out.look_table_dims, out.look_table_data,
                    out.look_table_count);
        out.look_table_encoding = profile.LookTableEncoding();
    }

    // The GPU render resamples this (deptheos_render's dng_render).
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
            out.tone_curve[i] = (float)toneCurve->Evaluate(x);
        }
    }
}

namespace {
struct RenderParamsFree {
    void operator()(DtDngRenderParams *params) const { dt_dng_render_params_free(params); }
};
} // namespace

DtDngRenderParams *deptheos_compute_render_params(const DngImageMetadata &meta,
                                                  const std::optional<DngWhiteXY> &white) {
    deptheos_host host;
    AutoPtr<dng_negative> negative(
        rebuild_negative_from_metadata(host, meta, nullptr, 0, 0, 0));
    // Frees the struct and its arrays if a later allocation throws.
    std::unique_ptr<DtDngRenderParams, RenderParamsFree> out(new DtDngRenderParams());
    std::memset(out.get(), 0, sizeof(DtDngRenderParams));
    fill_render_params(*negative, white, *out);
    return out.release();
}

std::optional<DngWhiteXY> deptheos_as_shot_white(const DngImageMetadata &meta) {
    deptheos_host host;
    AutoPtr<dng_negative> negative(
        rebuild_negative_from_metadata(host, meta, nullptr, 0, 0, 0));
    dng_camera_profile profile;
    std::unique_ptr<dng_color_spec> spec = make_color_spec(*negative, profile);
    if (!spec) return std::nullopt;
    set_white(*spec, *negative, std::nullopt);
    const dng_xy_coord &xy = spec->WhiteXY();
    return DngWhiteXY{xy.x, xy.y};
}

std::optional<std::vector<double>> deptheos_neutral_for_white(const DngImageMetadata &meta,
                                                              const DngWhiteXY &white) {
    deptheos_host host;
    AutoPtr<dng_negative> negative(
        rebuild_negative_from_metadata(host, meta, nullptr, 0, 0, 0));
    dng_camera_profile profile;
    std::unique_ptr<dng_color_spec> spec = make_color_spec(*negative, profile);
    if (!spec) return std::nullopt;
    set_white(*spec, *negative, white);
    // SetWhiteXY scales it to a largest of 1, each entry pinned to
    // [0.001, 1], as the SDK holds a camera neutral.
    const dng_vector &cw = spec->CameraWhite();
    std::vector<double> neutral(cw.Count());
    for (uint32 i = 0; i < cw.Count(); i++) neutral[i] = cw[i];
    return neutral;
}

std::optional<DngWhiteXY> deptheos_white_for_neutral(const DngImageMetadata &meta,
                                                     const double neutral[3]) {
    deptheos_host host;
    AutoPtr<dng_negative> negative(
        rebuild_negative_from_metadata(host, meta, nullptr, 0, 0, 0));
    dng_camera_profile profile;
    std::unique_ptr<dng_color_spec> spec = make_color_spec(*negative, profile);
    if (!spec || spec->Channels() != 3) return std::nullopt;
    dng_vector v(3);
    for (uint32 i = 0; i < 3; i++) v[i] = neutral[i];
    const dng_xy_coord xy = spec->NeutralToXY(v);
    return DngWhiteXY{xy.x, xy.y};
}

// A white the SDK takes: finite, and valid (dng_xy_coord::IsValid). The
// same test as src/lib.rs's `WhiteXy::is_valid`.
static bool is_valid(const DngWhiteXY &w) {
    return std::isfinite(w.x) && std::isfinite(w.y) && dng_xy_coord(w.x, w.y).IsValid();
}

void deptheos_temperature_of(const DngWhiteXY &white, double &kelvin, double &tint) {
    const dng_xy_coord xy = is_valid(white) ? dng_xy_coord(white.x, white.y)
                                                   : D50_xy_coord();
    const dng_temperature temperature(xy);
    kelvin = temperature.Temperature();
    tint = temperature.Tint();
}

DngWhiteXY deptheos_temperature_white(double kelvin, double tint) {
    const dng_xy_coord xy = std::isfinite(kelvin) && kelvin > 0 && std::isfinite(tint)
                                ? dng_temperature(kelvin, tint).Get_xy_coord()
                                : D50_xy_coord();
    return DngWhiteXY{xy.x, xy.y};
}

// ---------------------------------------------------------------------------
// Full image metadata for DNG write-back
// ---------------------------------------------------------------------------

static void extract_profile_luts(const dng_camera_profile &profile, DngLUTData &luts) {
    luts.hueSatMapEncoding = profile.HueSatMapEncoding();
    luts.lookTableEncoding = profile.LookTableEncoding();
    luts.defaultBlackRender = profile.DefaultBlackRender();
    const dng_srational &offset = profile.BaselineExposureOffset();
    luts.baselineExposureOffset = offset.d != 0 ? offset.As_real64() : 0.0;

    if (profile.HasHueSatDeltas()) {
        luts.hueSatMapDims1 = extract_hue_sat_dims(profile.HueSatDeltas1());
        luts.hueSatMapData1 = extract_hue_sat_floats(profile.HueSatDeltas1());
    }
    // The second map goes with the second illuminant, so it counts only in a
    // dual-illuminant profile.
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

// A camera profile's own fields. `luts` is replaced whole; `cs`'s negative
// fields (camera calibration, as-shot neutral, baseline exposure) stay.
static void extract_profile(const dng_camera_profile &profile, DngColorScienceInfo &cs,
                            DngLUTData &luts) {
    cs.colorMatrix1 = extract_matrix(profile.ColorMatrix1());
    cs.colorMatrix2 = extract_matrix(profile.ColorMatrix2());
    cs.forwardMatrix1 = extract_matrix(profile.ForwardMatrix1());
    cs.forwardMatrix2 = extract_matrix(profile.ForwardMatrix2());
    cs.calibrationIlluminant1 = profile.CalibrationIlluminant1();
    cs.calibrationIlluminant2 = profile.CalibrationIlluminant2();
    cs.profileCalibrationSig = safe_string(profile.ProfileCalibrationSignature().Get());
    cs.profileName = safe_string(profile.Name().Get());
    luts = DngLUTData();
    extract_profile_luts(profile, luts);
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
        extract_profile(negative.ProfileByIndex(0), out.colorScience, out.luts);
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
        out.camera.software = safe_string(exif->fSoftware.Get());
        out.camera.artist = safe_string(exif->fArtist.Get());
        out.camera.copyright = safe_string(exif->fCopyright.Get());

        // Lens
        out.lens.make = safe_string(exif->fLensMake.Get());
        out.lens.model = safe_string(exif->fLensName.Get());
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
    }
}

// ---------------------------------------------------------------------------
// DCP (DNG Camera Profile) loading
// ---------------------------------------------------------------------------

void deptheos_apply_dcp(DngImageMetadata &meta, const std::string &dcp_path) {
    utf8_file_stream stream(dcp_path);
    dng_camera_profile profile;

    if (!profile.ParseExtended(stream)) {
        dng_log(DNG_LOG_WARN, "[apply_dcp] ParseExtended failed for %s", dcp_path.c_str());
        ThrowBadFormat();
    }

    dng_log(DNG_LOG_INFO, "[apply_dcp] Loaded profile '%s' from %s",
            profile.Name().Get(), dcp_path.c_str());

    // Built on a copy so a throw part-way leaves `meta` as it was.
    DngImageMetadata md = meta;
    extract_profile(profile, md.colorScience, md.luts);
    meta = std::move(md);
}
