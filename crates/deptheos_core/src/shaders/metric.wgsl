// What both stackers' focus metrics share. No bindings.

#define_import_path deptheos::metric

// Dark floor of the Weber normalization |a − b| / (DARK_FLOOR + brightness):
// finite in shadow, yet dark and bright compete on relative contrast.
const DARK_FLOOR: f32 = 0.1;

// σ, and the taps either side of the center. `focus::BlurParams` in Rust.
struct BlurParams {
    sigma: f32,
    half_width: u32,
}

// Gaussian tap weight; σ is floored so σ = 0 isn't NaN.
fn gaussian_weight(offset: f32, sigma: f32) -> f32 {
    let s = max(sigma, 1e-3);
    return exp(-(offset * offset) / (2.0 * s * s));
}
