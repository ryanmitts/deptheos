//! The one image type the aligners work on: a row-major f32 gray plane.

/// A `width`×`height` image of one channel, `data` row-major. Values are
/// usually in `[0, 1]` but nothing here requires it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    /// A zero-filled plane.
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }

    /// `data.len()` must be `width·height` (checked in debug builds).
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Self {
        debug_assert_eq!(data.len(), width * height);
        Self { width, height, data }
    }

    /// Standard deviation of the values, shifted by the first so the sums
    /// stay small.
    pub fn std(&self) -> f32 {
        let Some(&shift) = self.data.first() else {
            return 0.0;
        };
        let (mut sum, mut sum_sq) = (0.0f64, 0.0f64);
        for &v in &self.data {
            let d = (v - shift) as f64;
            sum += d;
            sum_sq += d * d;
        }
        let n = self.data.len() as f64;
        let mean = sum / n;
        (sum_sq / n - mean * mean).max(0.0).sqrt() as f32
    }

    /// The value at column `x`, row `y`.
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Row `y`, `width` values.
    #[inline]
    pub fn row(&self, y: usize) -> &[f32] {
        &self.data[y * self.width..(y + 1) * self.width]
    }

    /// The `width`×`height` window at `(x0, y0)`, copied.
    #[cfg(test)]
    pub fn crop(&self, x0: usize, y0: usize, width: usize, height: usize) -> Self {
        let mut data = Vec::with_capacity(width * height);
        for y in y0..y0 + height {
            data.extend_from_slice(&self.data[y * self.width + x0..y * self.width + x0 + width]);
        }
        Self { width, height, data }
    }
}
