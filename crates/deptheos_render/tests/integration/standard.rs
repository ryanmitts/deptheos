//! A standard stack through the adjusted path, with neutral values: within
//! a set bound of the picture it shows unadjusted, for profiles of matrix
//! and curves and a table-based one, in its own profile and in sRGB.

use deptheos_core::io::IccProfile;
use deptheos_gpu::GpuContext;
use deptheos_render::dng_render::{Constants, DngRenderPipeline, PreparedRender, Region, Source};
use deptheos_render::{Adjustments, Color, OutputSpace, resolve_adjusted};
use image::{DynamicImage, ImageBuffer, Rgb};
use moxcms::{
    ColorProfile, LutMultidimensionalType, LutStore, LutWarehouse, ToneReprCurve, Vector3d,
};

const SIZE: [u32; 2] = [300, 200];

/// Gradients over every channel, saturated patches and a hashed texture,
/// so dark, bright and out-of-gamut values all appear.
fn scene() -> Vec<u16> {
    let [w, h] = SIZE;
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let hash = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663);
            let rgb: [u32; 3] = match (x / 50, y / 100) {
                (0, _) => [x * 1310, y * 327, (x + y) * 130],
                (1, 0) => [65_535, 0, 0],
                (1, 1) => [0, 65_535, 0],
                (2, 0) => [0, 0, 65_535],
                (2, 1) => [65_535, 65_535, 65_535],
                _ => [hash % 65_536, (hash >> 8) % 65_536, (hash >> 16) % 65_536],
            };
            pixels.extend(rgb.map(|v| v.min(65_535) as u16));
        }
    }
    pixels
}

fn profile(profile: ColorProfile) -> IccProfile {
    IccProfile::from_bytes(profile.encode().expect("encode")).expect("an RGB profile")
}

/// sRGB described by an A-to-B table instead of its matrix and curves.
fn table_based() -> IccProfile {
    let srgb = ColorProfile::new_srgb();
    let identity = || ToneReprCurve::Lut(Vec::new());
    let corners: Vec<u16> =
        (0..8u16).flat_map(|i| [i >> 2 & 1, i >> 1 & 1, i & 1].map(|c| c * 65_535)).collect();
    let mut grid = [0u8; 16];
    grid[..3].copy_from_slice(&[2, 2, 2]);
    let lut = LutMultidimensionalType {
        num_input_channels: 3,
        num_output_channels: 3,
        grid_points: grid,
        clut: Some(LutStore::Store16(corners)),
        a_curves: vec![srgb.red_trc.clone().unwrap(); 3],
        b_curves: vec![identity(), identity(), identity()],
        m_curves: vec![identity(), identity(), identity()],
        // The PCS's XYZ encoding: 1.0 is 32768/65535.
        matrix: {
            let mut m = srgb.colorant_matrix();
            m.v.iter_mut().flatten().for_each(|v| *v *= 32768.0 / 65535.0);
            m
        },
        bias: Vector3d { v: [0.0; 3] },
    };
    let mut table = srgb;
    table.lut_a_to_b_colorimetric = Some(LutWarehouse::Multidimensional(lut.clone()));
    table.lut_a_to_b_perceptual = Some(LutWarehouse::Multidimensional(lut));
    profile(table)
}

/// `pixels` drawn through `constants`, as RGB codes.
fn draw(ctx: &GpuContext, constants: &Constants, pixels: &[u16], region: Region) -> Vec<u16> {
    let [w, h] = SIZE;
    let input = match constants.linearizer() {
        Some(linearizer) => linearizer.upload(ctx, pixels, w, h),
        None => {
            let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, pixels.to_vec()).unwrap();
            crate::common::upload(ctx, &DynamicImage::ImageRgb16(buffer))
        }
    };
    let output = DngRenderPipeline::create_output(ctx, region.width, region.height);
    let prepared = PreparedRender::new(ctx, constants);
    DngRenderPipeline::new(ctx).draw(ctx, &prepared, Source::whole(&input), region, &output);
    let rgba = crate::common::download_rgba(ctx, &output);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .map(|v| (v * 65535.0).round() as u16)
        .collect()
}

/// The largest difference between two pictures, in 16-bit codes.
fn largest(a: &[u16], b: &[u16]) -> u16 {
    a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
}

/// How far the adjusted path with neutral values may be from the
/// unadjusted picture, in 16-bit codes. In its own profile a stack
/// round-trips within f32's error, a few codes where a pure power curve is
/// flat near black (Adobe RGB, 8). In sRGB it lands within the CPU
/// conversion's own rounding (20).
const OWN_BOUND: u16 = 16;
const SRGB_BOUND: u16 = 32;
/// A table-based profile, linearized on the CPU, against the CPU display:
/// both interpolate its table, differently. The test's table is coarse
/// (2 points a side), where moxcms's 16-bit and f32 routes differ from
/// each other by as much (493 measured).
const TABLE_BOUND: u16 = 512;

#[test]
fn neutral_draws_as_unadjusted_within_a_bound() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = scene();
    let whole = Region::whole(SIZE[0], SIZE[1]);
    let neutral = Adjustments::NEUTRAL;
    let profiles = [
        ("sRGB", None),
        ("Display P3", Some(profile(ColorProfile::new_display_p3()))),
        ("Adobe RGB", Some(IccProfile::adobe_rgb_compatible())),
        ("table-based", Some(table_based())),
    ];
    for (name, icc) in &profiles {
        let color = Color::Standard(icc.as_ref());
        let own = resolve_adjusted(color, &neutral, OutputSpace::StackProfile);
        let srgb = resolve_adjusted(color, &neutral, OutputSpace::Srgb);
        let table = own.linearizer().is_some();
        assert_eq!(table, *name == "table-based", "{name}");

        let unadjusted_srgb = match icc.as_ref().and_then(IccProfile::to_srgb) {
            Some(to_srgb) => {
                let mut out = vec![0; pixels.len()];
                to_srgb.convert(&pixels, &mut out);
                out
            }
            None => pixels.clone(),
        };
        let drawn_srgb = draw(&ctx, &srgb, &pixels, whole);
        let off_srgb = largest(&drawn_srgb, &unadjusted_srgb);

        if table {
            // Its own space has no curves to encode back through.
            assert_eq!(own.output(), OutputSpace::Srgb, "{name}");
        } else {
            assert_eq!(own.output(), OutputSpace::StackProfile, "{name}");
            let off_own = largest(&draw(&ctx, &own, &pixels, whole), &pixels);
            assert!(off_own <= OWN_BOUND, "{name}: its own profile off by {off_own}");
        }
        let bound = if table { TABLE_BOUND } else { SRGB_BOUND };
        assert!(off_srgb <= bound, "{name}: sRGB off by {off_srgb}");
    }
}

/// On a Display P3 screen a standard stack draws in P3 through the
/// adjusted path as the CPU converts it, so a settle doesn't shift color,
/// and a P3 stack keeps the colors sRGB can't show.
#[test]
fn neutral_draws_into_a_display_p3_screen_as_the_cpu_converts() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = scene();
    let whole = Region::whole(SIZE[0], SIZE[1]);
    let p3 = profile(ColorProfile::new_display_p3());
    let profiles = [("sRGB", None), ("Display P3", Some(p3.clone()))];
    for (name, icc) in &profiles {
        let constants = resolve_adjusted(
            Color::Standard(icc.as_ref()),
            &Adjustments::NEUTRAL,
            OutputSpace::DisplayP3,
        );
        assert_eq!(constants.output(), OutputSpace::DisplayP3, "{name}");
        let to_p3 = match icc {
            Some(icc) => icc.to_display_p3(),
            None => deptheos_core::io::RgbConversion::srgb_to_display_p3(),
        };
        let mut converted = vec![0; pixels.len()];
        to_p3.expect("a conversion to P3").convert(&pixels, &mut converted);
        let drawn = draw(&ctx, &constants, &pixels, whole);
        let off = largest(&drawn, &converted);
        assert!(off <= SRGB_BOUND, "{name}: Display P3 off by {off}");
    }
    // Pure P3 red stays pure red: nothing clips it to sRGB.
    let red = [65535u16, 0, 0].repeat((SIZE[0] * SIZE[1]) as usize);
    let constants =
        resolve_adjusted(Color::Standard(Some(&p3)), &Adjustments::NEUTRAL, OutputSpace::DisplayP3);
    let drawn = draw(&ctx, &constants, &red, whole);
    assert!(largest(&drawn, &red) <= OWN_BOUND, "P3 red clipped");
}

/// A region of a standard stack draws as that region of the whole.
#[test]
fn a_region_draws_as_the_whole_picture_does() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = scene();
    let p3 = profile(ColorProfile::new_display_p3());
    let neutral = Adjustments::NEUTRAL;
    for output in [OutputSpace::StackProfile, OutputSpace::Srgb] {
        let constants = resolve_adjusted(Color::Standard(Some(&p3)), &neutral, output);
        let full = draw(&ctx, &constants, &pixels, Region::whole(SIZE[0], SIZE[1]));
        let region = Region { x: 37, y: 51, width: 123, height: 77 };
        let drawn = draw(&ctx, &constants, &pixels, region);
        let cut: Vec<u16> = (region.y..region.y + region.height)
            .flat_map(|y| {
                let start = ((y * SIZE[0] + region.x) * 3) as usize;
                full[start..start + (region.width * 3) as usize].to_vec()
            })
            .collect();
        assert!(drawn == cut, "{output:?}: the region differs");
    }
}
