// sRGB ↔ linear: the exact piecewise IEC 61966-2-1 curves.
//
// Focus metrics and the Laplacian pyramid run on sRGB-encoded values: in
// linear space highlights weigh too much and darken the result. The output
// stays linear for writing back to DNG.

#define_import_path deptheos::color

fn linear_to_srgb_channel(c: f32) -> f32 {
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear_channel(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb3(c: vec3f) -> vec3f {
    return vec3f(
        linear_to_srgb_channel(c.r),
        linear_to_srgb_channel(c.g),
        linear_to_srgb_channel(c.b),
    );
}

fn srgb_to_linear3(c: vec3f) -> vec3f {
    return vec3f(
        srgb_to_linear_channel(c.r),
        srgb_to_linear_channel(c.g),
        srgb_to_linear_channel(c.b),
    );
}
