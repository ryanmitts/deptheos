use image::math::Rect;
use nalgebra::{Affine2, Point2};

/// The rectangle arithmetic the aligners need on `image`'s [`Rect`]: the
/// valid area every frame covers, kept by [`crate::Aligner`].
pub trait RectExt {
    /// The overlap of two rectangles; zero-sized when they do not meet.
    fn intersection(&self, rect2: Rect) -> Rect;
    /// The axis-aligned rectangle inside this one's image under
    /// `transform`, clipped to the positive quadrant; exact for any
    /// orientation-preserving transform rotating under 45°.
    fn inside_after_transform(&self, transform: &Affine2<f32>) -> Rect;
}

impl RectExt for Rect {
    fn intersection(&self, rect2: Rect) -> Rect {
        let x0 = std::cmp::max(self.x, rect2.x);
        let y0 = std::cmp::max(self.y, rect2.y);
        let x1 = std::cmp::min(self.x + self.width, rect2.x + rect2.width);
        let y1 = std::cmp::min(self.y + self.height, rect2.y + rect2.height);
        Rect { x: x0, y: y0, width: x1.saturating_sub(x0), height: y1.saturating_sub(y0) }
    }

    fn inside_after_transform(&self, transform: &Affine2<f32>) -> Rect {
        let tl = transform.transform_point(&Point2::new(self.x as f32, self.y as f32));
        let tr =
            transform.transform_point(&Point2::new((self.x + self.width) as f32, self.y as f32));
        let bl =
            transform.transform_point(&Point2::new(self.x as f32, (self.y + self.height) as f32));
        let br = transform.transform_point(&Point2::new(
            (self.x + self.width) as f32,
            (self.y + self.height) as f32,
        ));

        let top = tl.y.ceil().max(tr.y.ceil()) as i32;
        let left = tl.x.ceil().max(bl.x.ceil()) as i32;
        let bottom = bl.y.floor().min(br.y.floor()) as i32;
        let right = br.x.floor().min(tr.x.floor()) as i32;

        let left = left.max(0) as u32;
        let top = top.max(0) as u32;
        let right = right.max(0) as u32;
        let bottom = bottom.max(0) as u32;

        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);

        Rect { x: left, y: top, width, height }
    }
}

/// `transform` re-expressed on a grid resized by `(sx, sy)` with pixel
/// centers kept (`x' = s·(x + 0.5) − 0.5`, as every resizer places them).
/// The translation picks up `(I − A)·(s − 1)/2`, which a chain would
/// otherwise accumulate as drift. The inverse is `(1/sx, 1/sy)`.
pub(crate) fn rescale_affine(transform: &Affine2<f32>, sx: f64, sy: f64) -> Affine2<f32> {
    let c =
        nalgebra::Matrix3::new(sx, 0.0, (sx - 1.0) / 2.0, 0.0, sy, (sy - 1.0) / 2.0, 0.0, 0.0, 1.0);
    let c_inv = nalgebra::Matrix3::new(
        1.0 / sx,
        0.0,
        (1.0 / sx - 1.0) / 2.0,
        0.0,
        1.0 / sy,
        (1.0 / sy - 1.0) / 2.0,
        0.0,
        0.0,
        1.0,
    );
    Affine2::from_matrix_unchecked((c * transform.matrix().cast::<f64>() * c_inv).cast::<f32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescale_round_trips_and_matches_point_mapping() {
        let t = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.004, 0.002, 3.5, -0.001, 0.997, -2.25, 0.0, 0.0, 1.0,
        ));
        let (sx, sy) = (0.25, 0.2501);
        let level = rescale_affine(&t, sx, sy);
        // A point mapped at full resolution and carried to the level equals
        // the point carried to the level and mapped there.
        let p = Point2::new(1234.0f32, 567.0);
        let to_level = |q: Point2<f32>| {
            Point2::new(sx as f32 * (q.x + 0.5) - 0.5, sy as f32 * (q.y + 0.5) - 0.5)
        };
        let a = to_level(t.transform_point(&p));
        let b = level.transform_point(&to_level(p));
        assert!((a - b).norm() < 1e-3, "{a} vs {b}");
        let back = rescale_affine(&level, 1.0 / sx, 1.0 / sy);
        assert!((back.matrix() - t.matrix()).abs().max() < 1e-4);
    }
}
