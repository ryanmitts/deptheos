// Counts the pixels of a drawn picture's window whose channels show as
// 8-bit 0 or 255, per channel: the clipping the histogram flags, from
// every pixel, so a highlight a pixel wide counts (ADR-138).

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> counts: array<atomic<u32>, 6>;
@group(0) @binding(2) var<uniform> window: vec4u;

// u16 codes that round to 8-bit 0 and 255 (`io::u16_to_u8`).
const BLACK: u32 = 128u;
const WHITE: u32 = 65407u;

var<workgroup> local: array<atomic<u32>, 6>;

@compute @workgroup_size(16, 16)
fn clip(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) i: u32) {
    if (i < 6u) {
        atomicStore(&local[i], 0u);
    }
    workgroupBarrier();
    if (id.x < window.z && id.y < window.w) {
        let v = textureLoad(picture, vec2i(window.xy + id.xy), 0).rgb;
        let code = vec3u(clamp(v, vec3f(0.0), vec3f(1.0)) * 65535.0 + 0.5);
        for (var c = 0u; c < 3u; c++) {
            if (code[c] <= BLACK) {
                atomicAdd(&local[c], 1u);
            }
            if (code[c] >= WHITE) {
                atomicAdd(&local[3u + c], 1u);
            }
        }
    }
    workgroupBarrier();
    if (i < 6u) {
        let n = atomicLoad(&local[i]);
        if (n > 0u) {
            atomicAdd(&counts[i], n);
        }
    }
}
