// A scalar map in a storage buffer (`scalar.rs`). `fill` starts one at a
// value not zero; `clear_buffer` covers the rest.

struct FillParams {
    value: f32,
    // The map's row length.
    width: u32,
}

@group(0) @binding(0) var<storage, read_write> fill_dst: array<f32>;
@group(0) @binding(1) var<uniform> fill_params: FillParams;

@compute @workgroup_size(16, 16)
fn fill(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.y * fill_params.width + gid.x;
    if (gid.x >= fill_params.width || i >= arrayLength(&fill_dst)) {
        return;
    }
    fill_dst[i] = fill_params.value;
}
