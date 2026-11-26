//! DCP profile discovery and application against the installed Adobe
//! profiles, skipped where there are none (CI). Tests the GPU's
//! HueSatMap/LookTable paths against the SDK's CPU render with an Adobe
//! Standard profile (linear tables) and a camera-matching one (an
//! sRGB-encoded LookTable, DefaultBlackRender None, BaselineExposureOffset).

use deptheos_core::dcp::{self, DcpPreference};
use deptheos_dng::DngImage;
use deptheos_gpu::GpuContext;
use deptheos_raw::{RawDngColor, RawMetadata, TestMetadata};

use crate::dng_render::assert_renders_like_sdk;

fn xt5_metadata() -> TestMetadata {
    let mut color = RawDngColor { illuminant: 21, ..Default::default() };
    #[rustfmt::skip]
    let xyz_to_cam = [
        1.3426, -0.6334, -0.1177,
        -0.4244, 1.2136, 0.2371,
        -0.0580, 0.1303, 0.6122,
        0.0, 0.0, 0.0,
    ];
    color.color_matrix = Some(xyz_to_cam);
    TestMetadata {
        make: "FUJIFILM".into(),
        model: "X-T5".into(),
        unique_camera_model: "FUJIFILM X-T5".into(),
        cam_mul: [2.2, 1.0, 1.6, 1.0],
        raw_colors: 3,
        dng_color: [color, RawDngColor::default()],
        ..Default::default()
    }
}

/// A Canon whose camera-matching profiles ship with Camera Raw. The color
/// matrix is the X-T5's: the profile replaces it.
fn r5_mark_ii_metadata() -> TestMetadata {
    TestMetadata {
        make: "Canon".into(),
        model: "EOS R5 Mark II".into(),
        unique_camera_model: "Canon EOS R5 Mark II".into(),
        ..xt5_metadata()
    }
}

#[test]
fn dcp_profile_applies_and_renders_with_luts() {
    check_dcp_render(&xt5_metadata().into(), DcpPreference::AdobeStandard, "Adobe Standard");
}

#[test]
fn camera_matching_profile_renders_like_the_sdk() {
    check_dcp_render(
        &r5_mark_ii_metadata().into(),
        DcpPreference::CameraStandard,
        "Camera Standard",
    );
}

/// Applies `metadata`'s camera's `preference` profile (skipping unless the
/// one found is named `expected`) and checks the GPU render against the
/// SDK's CPU render of the same pixels and profile.
fn check_dcp_render(metadata: &RawMetadata, preference: DcpPreference, expected: &str) {
    let model = metadata.unique_camera_model();
    let Some(profile_path) =
        dcp::find_profile(&model, preference).filter(|p| p.to_string_lossy().contains(expected))
    else {
        eprintln!("skipping: no {expected} profile installed for '{model}'");
        return;
    };
    println!("using profile {}", profile_path.display());

    let mut source = DngImage::from_raw_metadata(metadata).expect("metadata synthesis");
    source.apply_dcp(&profile_path).expect("apply DCP");

    let render_params = source.render_params().expect("render params");
    assert!(
        render_params.hue_sat_map.is_some() || render_params.look_table.is_some(),
        "{expected} profiles carry a HueSatMap or a LookTable"
    );

    // Full-pipeline parity with real profile LUTs: GPU vs SDK CPU render.
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    assert_renders_like_sdk(&ctx, &source, &format!("{expected} profile"));
}
