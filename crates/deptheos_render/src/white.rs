//! A standard stack's white balance: one adaptation matrix in linear
//! ProPhoto, from the white the relative scale names to the scale's
//! reference white, scaled so no channel of white gains above 1.
//!
//! The locus and tint are lightcraft's, as they stand (MIT OR Apache-2.0:
//! craft-libs b2d2a1e94 `crates/color-transform/src/cct.rs`), and so is
//! the checked Bradford adaptation, which gives the identity where a
//! white's cone response is not above 0 (craft-libs b2d2a1e94
//! `crates/cms/src/math.rs`, `bradford_checked`).

use deptheos_adjust::temperature::{MIREDS_PER_STEP, REFERENCE_KELVIN, mireds, relative_to_kelvin};

pub(crate) type Mat3 = [[f64; 3]; 3];

pub(crate) const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Tint units per unit of Duv.
const TINT_SCALE: f64 = 3000.0;
/// The range the locus's cubic approximation covers, in kelvins.
const KIM_MIN: f64 = 1667.0;
const KIM_MAX: f64 = 25000.0;
/// The range of temperatures taken.
const MIN_TEMPERATURE: f64 = 1000.0;
const MAX_TEMPERATURE: f64 = 100_000.0;

/// Bradford's cone response matrix (Lam 1985): XYZ to sharpened cones.
const BRADFORD: Mat3 =
    [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// ProPhoto's primaries and its D50 white, as xy.
pub(crate) const PROPHOTO: [[f64; 2]; 4] =
    [[0.734699, 0.265301], [0.159597, 0.840403], [0.036598, 0.000105], [0.3457, 0.3585]];
/// Rec. 2020's primaries and its D65 white, as xy.
const REC2020: [[f64; 2]; 4] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], [0.3127, 0.3290]];

/// The matrix, in linear ProPhoto, for a standard stack's relative
/// `temperature` and `tint`: positive temperature warmer, positive tint
/// toward magenta. 0 and 0 give the identity.
pub(crate) fn relative_matrix(temperature: f64, tint: f64) -> Mat3 {
    let set = white_xyz(relative_to_kelvin(temperature), tint);
    let reference = white_xyz(REFERENCE_KELVIN, 0.0);
    let Some(adapt) = bradford_checked(set, reference) else { return IDENTITY };
    let (to_xyz, from_xyz) = prophoto();
    let m = mul(&from_xyz, &mul(&adapt, &to_xyz));
    // White's largest channel stays 1, so white keeps its tint unclipped.
    let white = apply(&m, [1.0; 3]);
    let top = white.into_iter().fold(0.0, f64::max);
    if !(top.is_finite() && top > 0.0) {
        return IDENTITY;
    }
    m.map(|row| row.map(|v| v / top))
}

/// The relative temperature and tint whose [`relative_matrix`] draws
/// `color` (linear ProPhoto, as shot) as a gray: the white it was lit
/// by, found from the von Kries law the matrix applies, then on the locus.
/// `None` for a color with no cone response, or off the scale's reach.
/// Not clamped to the sliders' ranges.
pub fn relative_for_gray(color: [f64; 3]) -> Option<(f64, f64)> {
    let (to_xyz, _) = prophoto();
    let cone = |v: [f64; 3]| apply(&BRADFORD, apply(&to_xyz, v));
    let (lit, gray) = (cone(color), cone([1.0; 3]));
    let reference = apply(&BRADFORD, white_xyz(REFERENCE_KELVIN, 0.0));
    if !lit.iter().chain(&gray).all(|c| c.is_finite() && *c > 0.0) {
        return None;
    }
    let set: [f64; 3] = std::array::from_fn(|i| reference[i] * lit[i] / gray[i]);
    let xyz = apply(&invert(&BRADFORD)?, set);
    let sum: f64 = xyz.iter().sum();
    if !(sum.is_finite() && sum > 0.0 && xyz[1] > 0.0) {
        return None;
    }
    let (kelvin, tint) = locus_of([xyz[0] / sum, xyz[1] / sum])?;
    Some((kelvin_to_relative_unclamped(kelvin), tint))
}

/// [`kelvin_to_relative`] past its range.
fn kelvin_to_relative_unclamped(kelvin: f64) -> f64 {
    (mireds(REFERENCE_KELVIN) - mireds(kelvin)) / MIREDS_PER_STEP
}

/// The temperature and tint [`temp_tint_to_xy`] takes to `xy`, by Newton's
/// method in mireds and tint; `None` if it doesn't converge in range.
fn locus_of(xy: [f64; 2]) -> Option<(f64, f64)> {
    let (u, v) = xy_to_uv(xy);
    let at = |m: f64, tint: f64| xy_to_uv(temp_tint_to_xy(1e6 / m, tint));
    let (mut m, mut tint) = (mireds(REFERENCE_KELVIN), 0.0);
    for _ in 0..50 {
        let (fu, fv) = at(m, tint);
        let (eu, ev) = (fu - u, fv - v);
        if eu.hypot(ev) < 1e-12 {
            return (10.0..=1000.0).contains(&m).then_some((1e6 / m, tint));
        }
        let (h, k) = (1e-3, 1e-3);
        let (mu, mv) = at(m + h, tint);
        let (tu, tv) = at(m, tint + k);
        let j = [[(mu - fu) / h, (tu - fu) / k], [(mv - fv) / h, (tv - fv) / k]];
        let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
        if !det.is_finite() || det.abs() < 1e-18 {
            return None;
        }
        m -= (j[1][1] * eu - j[0][1] * ev) / det;
        tint -= (j[0][0] * ev - j[1][0] * eu) / det;
        m = m.clamp(5.0, 2000.0);
    }
    None
}

fn kim_xy(t: f64) -> [f64; 2] {
    let t = t.clamp(KIM_MIN, KIM_MAX);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    [x, y]
}

/// CIE 1931 xy to CIE 1960 (u, v), and back.
fn xy_to_uv([x, y]: [f64; 2]) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy(u: f64, v: f64) -> [f64; 2] {
    let d = 2.0 * u - 8.0 * v + 4.0;
    [3.0 * u / d, 2.0 * v / d]
}

/// The locus in (u, v) at `t` kelvins, continued straight in mireds
/// outside the approximation's range.
fn locus_uv(t: f64) -> (f64, f64) {
    if (KIM_MIN..=KIM_MAX).contains(&t) {
        return xy_to_uv(kim_xy(t));
    }
    let (a, b) = if t < KIM_MIN { (KIM_MIN, KIM_MIN + 50.0) } else { (KIM_MAX - 500.0, KIM_MAX) };
    let (ua, va) = xy_to_uv(kim_xy(a));
    let (ub, vb) = xy_to_uv(kim_xy(b));
    let (ma, mb, m) = (1e6 / a, 1e6 / b, 1e6 / t);
    let s = (m - ma) / (mb - ma);
    (ua + (ub - ua) * s, va + (vb - va) * s)
}

/// The locus's unit normal at `t`, toward green (increasing v).
fn locus_normal(t: f64) -> (f64, f64) {
    let m = 1e6 / t;
    let (u0, v0) = locus_uv(1e6 / (m + 1.0));
    let (u1, v1) = locus_uv(1e6 / (m - 1.0).max(1.0));
    let (du, dv) = (u1 - u0, v1 - v0);
    let l = du.hypot(dv).max(1e-12);
    let (nx, ny) = (-dv / l, du / l);
    if ny < 0.0 { (-nx, -ny) } else { (nx, ny) }
}

/// The white at `t` kelvins and `tint`, as xy. A positive tint is a white
/// toward green, whose correction adds magenta.
fn temp_tint_to_xy(t: f64, tint: f64) -> [f64; 2] {
    let t = if t.is_nan() { 6500.0 } else { t.clamp(MIN_TEMPERATURE, MAX_TEMPERATURE) };
    let tint = if tint.is_finite() { tint } else { 0.0 };
    let (u, v) = locus_uv(t);
    let (nu, nv) = locus_normal(t);
    let duv = tint / TINT_SCALE;
    uv_to_xy(u + nu * duv, v + nv * duv)
}

pub(crate) fn xy_to_xyz([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn white_xyz(t: f64, tint: f64) -> [f64; 3] {
    xy_to_xyz(temp_tint_to_xy(t, tint))
}

/// The Bradford adaptation from white `src` to white `dst`, or `None`
/// where a white's cone response is not finite and above 0.
pub(crate) fn bradford_checked(src: [f64; 3], dst: [f64; 3]) -> Option<Mat3> {
    let (s, d) = (apply(&BRADFORD, src), apply(&BRADFORD, dst));
    let ok = |v: [f64; 3]| v.iter().all(|c| c.is_finite() && *c > 0.0);
    if !(ok(s) && ok(d)) {
        return None;
    }
    let scale = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    let m = mul(&invert(&BRADFORD)?, &mul(&scale, &BRADFORD));
    m.iter().flatten().all(|v| v.is_finite()).then_some(m)
}

/// Linear ProPhoto to XYZ, and back.
pub(crate) fn prophoto() -> (Mat3, Mat3) {
    rgb_to_xyz(PROPHOTO)
}

/// Linear ProPhoto to linear Rec. 2020, white adapted D50 to D65
/// (Bradford), as lightcraft converts (craft-libs b2d2a1e94
/// `crates/develop/src/light_v3.rs`, `prophoto_matrices`).
pub(crate) fn prophoto_to_rec2020() -> Mat3 {
    let (to_xyz, _) = prophoto();
    let (_, from_xyz) = rgb_to_xyz(REC2020);
    let adapt =
        bradford_checked(xy_to_xyz(PROPHOTO[3]), xy_to_xyz(REC2020[3])).expect("D50 and D65 adapt");
    mul(&from_xyz, &mul(&adapt, &to_xyz))
}

/// A space's linear values to XYZ, and back, from its primaries and white.
fn rgb_to_xyz(space: [[f64; 2]; 4]) -> (Mat3, Mat3) {
    let [r, g, b, white] = space.map(xy_to_xyz);
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let inverse = invert(&m).expect("the primaries are independent");
    let s = apply(&inverse, white);
    let to_xyz = m.map(|row| [row[0] * s[0], row[1] * s[1], row[2] * s[2]]);
    (to_xyz, invert(&to_xyz).expect("the matrix inverts"))
}

pub(crate) fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

pub(crate) fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

pub(crate) fn invert(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let c = |i: usize, j: usize| {
        let (i0, i1, j0, j1) = ((i + 1) % 3, (i + 2) % 3, (j + 1) % 3, (j + 2) % 3);
        m[i0][j0] * m[i1][j1] - m[i0][j1] * m[i1][j0]
    };
    Some(std::array::from_fn(|i| std::array::from_fn(|j| c(j, i) / det)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat3, b: &Mat3, tolerance: f64) -> bool {
        a.iter().flatten().zip(b.iter().flatten()).all(|(a, b)| (a - b).abs() < tolerance)
    }

    #[test]
    fn prophoto_is_the_published_matrix() {
        let published = [
            [0.7976749, 0.1351917, 0.0313534],
            [0.2880402, 0.7118741, 0.0000857],
            [0.0, 0.0, 0.8252100],
        ];
        let (to_xyz, from_xyz) = prophoto();
        assert!(close(&to_xyz, &published, 2e-4), "{to_xyz:?}");
        assert!(close(&mul(&to_xyz, &from_xyz), &IDENTITY, 1e-12));
    }

    #[test]
    fn the_reference_is_the_identity_and_white_never_passes_1() {
        assert!(close(&relative_matrix(0.0, 0.0), &IDENTITY, 1e-12));
        for t in [-100.0, -30.0, 0.0, 30.0, 100.0] {
            for tint in [-100.0, 0.0, 100.0] {
                let white = apply(&relative_matrix(t, tint), [1.0; 3]);
                let top = white.into_iter().fold(0.0, f64::max);
                assert!((top - 1.0).abs() < 1e-12, "{t} {tint}: {white:?}");
                assert!(white.iter().all(|&c| c > 0.0), "{t} {tint}: {white:?}");
            }
        }
    }

    /// Positive temperature is warmer, positive tint more magenta: gray
    /// goes redder than blue, and greener than its red and blue fall.
    #[test]
    fn the_signs_go_warm_and_magenta() {
        let gray = |t, tint| apply(&relative_matrix(t, tint), [0.18; 3]);
        let [r, _, b] = gray(40.0, 0.0);
        assert!(r > b, "warmer: {r} {b}");
        let [r, _, b] = gray(-40.0, 0.0);
        assert!(r < b, "cooler: {r} {b}");
        let [r, g, b] = gray(0.0, 40.0);
        assert!(g < r && g < b, "magenta: {r} {g} {b}");
        let [r, g, b] = gray(0.0, -40.0);
        assert!(g > r && g > b, "green: {r} {g} {b}");
    }

    /// A gray lit by the white a relative value names gives that value
    /// back, and its matrix draws it gray again.
    #[test]
    fn a_lit_gray_gives_back_its_white_balance() {
        let (to_xyz, from_xyz) = prophoto();
        let reference = white_xyz(REFERENCE_KELVIN, 0.0);
        for t in [-100.0, -60.0, -15.0, 0.0, 25.0, 70.0, 100.0] {
            for tint in [-80.0, -20.0, 0.0, 35.0, 90.0] {
                // A gray under the light the value corrects: the inverse
                // adaptation of ProPhoto's gray.
                let set = white_xyz(relative_to_kelvin(t), tint);
                let lit = bradford_checked(reference, set).unwrap();
                let gray = apply(&mul(&from_xyz, &mul(&lit, &to_xyz)), [0.18; 3]);
                let (rt, rtint) = relative_for_gray(gray).expect("on the scale");
                assert!(
                    (rt - t).abs() < 1e-6 && (rtint - tint).abs() < 1e-6,
                    "{t} {tint}: {rt} {rtint}"
                );
                let drawn = apply(&relative_matrix(rt, rtint), gray);
                let spread = drawn.iter().fold(0.0f64, |a, &c| a.max((c - drawn[1]).abs()));
                assert!(spread < 1e-9, "{t} {tint}: {drawn:?}");
            }
        }
        assert_eq!(
            relative_for_gray([0.18; 3]).map(|(t, i)| (t.abs() < 1e-9, i.abs() < 1e-9)),
            Some((true, true))
        );
        assert_eq!(relative_for_gray([0.0; 3]), None);
        assert_eq!(relative_for_gray([f64::NAN, 0.2, 0.2]), None);
    }

    #[test]
    fn a_white_with_no_cone_response_adapts_nothing() {
        assert_eq!(bradford_checked([0.0; 3], white_xyz(6500.0, 0.0)), None);
        assert_eq!(bradford_checked([f64::NAN; 3], white_xyz(6500.0, 0.0)), None);
        let d65 = white_xyz(6500.0, 0.0);
        assert!(close(&bradford_checked(d65, d65).unwrap(), &IDENTITY, 1e-12));
    }
}
