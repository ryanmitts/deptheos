//! Run the ECC chain and the grid aligner over the same frames and print,
//! per frame, how far apart the two transforms place the frame's corners
//! and center. Usage: `compare_aligners <frames...>`.

use deptheos_aligner::{AlignMode, AlignedFrame, Aligner, GridConfig};
use image::DynamicImage;
use nalgebra::{Affine2, Point2};

/// A frame as the aligners read it: RGB8, or RGB16 for deeper sources.
fn load_rgb(path: &str) -> DynamicImage {
    let image = image::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let color = image.color();
    if color.bits_per_pixel() / color.channel_count() as u16 > 8 {
        DynamicImage::ImageRgb16(image.into_rgb16())
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
    }
}

fn scale_of(t: &Affine2<f32>) -> f32 {
    let m = t.matrix();
    (m[(0, 0)] * m[(1, 1)] - m[(0, 1)] * m[(1, 0)]).abs().sqrt()
}

fn rotation_of(t: &Affine2<f32>) -> f32 {
    let m = t.matrix();
    m[(1, 0)].atan2(m[(0, 0)]).to_degrees()
}

/// `LOOP_CLOSURE=1`: run each chain forward and backward and report how
/// far the round trip is from the identity: drift with no ground truth.
fn loop_closure(paths: &[String], config: GridConfig) {
    let images: Vec<DynamicImage> = paths.iter().map(|p| load_rgb(p)).collect();
    let (w, h) = (images[0].width() as f32, images[0].height() as f32);
    let run = |mut aligner: Aligner, order: &[usize]| -> Affine2<f32> {
        let mut last = Affine2::identity();
        for &i in order {
            last = aligner.align(&images[i]).expect("align").transform;
        }
        last
    };
    let forward: Vec<usize> = (0..images.len()).collect();
    let backward: Vec<usize> = (0..images.len()).rev().collect();
    type Make = Box<dyn Fn() -> Aligner>;
    let mut aligners: Vec<(&str, Make)> = Vec::new();
    if std::env::var("SKIP_ECC").is_err() {
        aligners.push(("ecc", Box::new(|| Aligner::new(AlignMode::Ecc)) as Make));
    }
    aligners.push((
        "grid",
        Box::new(move || Aligner::with_grid_config(AlignMode::Grid, config)) as Make,
    ));
    for (name, make) in aligners {
        let f = run(make(), &forward); // frame 0 → last
        let b = run(make(), &backward); // last → frame 0
        // Their composition should be the identity.
        let round = b * f;
        let c = Point2::new(w / 2.0, h / 2.0);
        let dc = round.transform_point(&c) - c;
        let corner = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .into_iter()
            .map(|(x, y)| {
                let p = Point2::new(x, y);
                (round.transform_point(&p) - p).norm()
            })
            .fold(0.0f32, f32::max);
        println!(
            "{name:>5}: round trip center ({:+.2}, {:+.2}) px, worst corner {:.2} px, scale {:.5}, rot {:+.4}°",
            dc.x,
            dc.y,
            corner,
            scale_of(&round),
            rotation_of(&round)
        );
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: compare_aligners <frames...>");
        std::process::exit(2);
    }
    let mut ecc = Aligner::new(AlignMode::Ecc);
    // Env override for experiments: GRID_ROT (degrees).
    let env_f32 = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
    let config = GridConfig {
        rotation_deg: env_f32("GRID_ROT").unwrap_or(GridConfig::default().rotation_deg),
        ..Default::default()
    };
    eprintln!("grid config: {config:?}");
    if std::env::var("LOOP_CLOSURE").is_ok() {
        return loop_closure(&paths, config);
    }
    let mut grid = Aligner::with_grid_config(AlignMode::Grid, config);
    println!(
        "frame  center_dx center_dy  corner_max  ecc_scale grid_scale  ecc_rot grid_rot  ecc_shear ecc_aniso | step: d_rot d_scale d_shift   t_ecc t_grid"
    );
    let mut prev: Option<(Affine2<f32>, Affine2<f32>)> = None;
    for (i, path) in paths.iter().enumerate() {
        let image = load_rgb(path);
        let (w, h) = (image.width() as f32, image.height() as f32);
        let t0 = std::time::Instant::now();
        let e: AlignedFrame = ecc.align(&image).expect("ecc");
        let t1 = t0.elapsed();
        let t0 = std::time::Instant::now();
        let g: AlignedFrame = grid.align(&image).expect("grid");
        let t2 = t0.elapsed();
        let c = Point2::new(w / 2.0, h / 2.0);
        let dc = g.transform.transform_point(&c) - e.transform.transform_point(&c);
        let corner_max = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .into_iter()
            .map(|(x, y)| {
                let p = Point2::new(x, y);
                (g.transform.transform_point(&p) - e.transform.transform_point(&p)).norm()
            })
            .fold(0.0f32, f32::max);
        // Shear: angle between the rotations implied by the two columns;
        // anisotropy: ratio of the column lengths.
        let m = e.transform.matrix();
        let rot_x = m[(1, 0)].atan2(m[(0, 0)]).to_degrees();
        let rot_y = (-m[(0, 1)]).atan2(m[(1, 1)]).to_degrees();
        let sx = (m[(0, 0)] * m[(0, 0)] + m[(1, 0)] * m[(1, 0)]).sqrt();
        let sy = (m[(0, 1)] * m[(0, 1)] + m[(1, 1)] * m[(1, 1)]).sqrt();
        // Per-step increments (previous frame → this frame) of each aligner.
        let step = prev.map(|(pe, pg)| {
            let ie = e.transform * pe.try_inverse().unwrap();
            let ig = g.transform * pg.try_inverse().unwrap();
            let dc = ig.transform_point(&c) - ie.transform_point(&c);
            (rotation_of(&ig) - rotation_of(&ie), scale_of(&ig) - scale_of(&ie), dc.norm())
        });
        prev = Some((e.transform, g.transform));
        let (d_rot, d_scale, d_shift) = step.unwrap_or((0.0, 0.0, 0.0));
        println!(
            "{i:>5}  {:>9.2} {:>9.2}  {:>10.2}  {:>9.5} {:>10.5}  {:>7.3} {:>8.3}  {:>9.3} {:>9.4} | {:>+7.4} {:>+8.5} {:>7.2}   {:>5.0?} {:>6.0?}",
            dc.x,
            dc.y,
            corner_max,
            scale_of(&e.transform),
            scale_of(&g.transform),
            rotation_of(&e.transform),
            rotation_of(&g.transform),
            rot_x - rot_y,
            sx / sy,
            d_rot,
            d_scale,
            d_shift,
            t1,
            t2
        );
    }
}
