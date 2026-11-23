// Standalone warp of a whole texture into the stack's (cropped) geometry,
// for compare and `deptheos align`. Lanczos-3, as the Pyramid and Wavelet
// stackers warp; exposure-corrected, no sRGB step, no margin weights.

#import deptheos::warp::{WarpParams, warp_lanczos}

@group(0) @binding(0) var warp_src: texture_2d<f32>;
#ifdef SOURCE_RGBA16UNORM
@group(0) @binding(1) var warp_dst: texture_storage_2d<rgba16unorm, write>;
#else
@group(0) @binding(1) var warp_dst: texture_storage_2d<rgba32float, write>;
#endif
@group(0) @binding(2) var<uniform> warp_params: WarpParams;

@compute @workgroup_size(16, 16)
fn warp_texture(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(warp_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    textureStore(warp_dst, gid.xy, warp_lanczos(warp_src, warp_params, gid.xy));
}
