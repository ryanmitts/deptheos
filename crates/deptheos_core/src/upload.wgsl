// Unpack raw RGB/RGBA bytes (8- or 16-bit channels) from a storage buffer
// into a texture, normalized to [0,1], with no color conversion.

struct UploadParams {
    width: u32,
    channels: u32,          // 3 (RGB) or 4 (RGBA)
    bytes_per_channel: u32, // 1 (u8) or 2 (u16)
}

@group(0) @binding(0) var<uniform> params: UploadParams;
@group(0) @binding(1) var<storage, read> pixel_data: array<u32>;

// One little-endian channel at a byte offset. A 16-bit channel's offset is
// even, so it never straddles a word.
fn read_channel(byte_offset: u32, bytes_per_channel: u32) -> f32 {
    let word_idx = byte_offset >> 2u;
    let byte_in_word = byte_offset & 3u;

    if (bytes_per_channel == 2u) {
        let raw = extractBits(pixel_data[word_idx], byte_in_word * 8u, 16u);
        return f32(raw) / 65535.0;
    } else {
        let raw = extractBits(pixel_data[word_idx], byte_in_word * 8u, 8u);
        return f32(raw) / 255.0;
    }
}

// The target follows the context's source_format(): Rgba16Unorm where the
// adapter stores it, else Rgba32Float; TARGET_RGBA8 for 8-bit frames. All
// exact for their inputs. (No double colons in WGSL comments: naga_oil
// rewrites module paths even there.)
#ifdef TARGET_RGBA8
@group(1) @binding(0) var rgba_out: texture_storage_2d<rgba8unorm, write>;
#else ifdef SOURCE_RGBA16UNORM
@group(1) @binding(0) var rgba_out: texture_storage_2d<rgba16unorm, write>;
#else
@group(1) @binding(0) var rgba_out: texture_storage_2d<rgba32float, write>;
#endif

@compute @workgroup_size(16, 16)
fn unpack_to_rgba(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(rgba_out);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let pixel_idx = gid.y * params.width + gid.x;
    let stride = params.channels * params.bytes_per_channel;
    let base = pixel_idx * stride;

    let r = read_channel(base, params.bytes_per_channel);
    let g = read_channel(base + params.bytes_per_channel, params.bytes_per_channel);
    let b = read_channel(base + 2u * params.bytes_per_channel, params.bytes_per_channel);

    var a: f32 = 1.0;
    if (params.channels == 4u) {
        a = read_channel(base + 3u * params.bytes_per_channel, params.bytes_per_channel);
    }

    textureStore(rgba_out, gid.xy, vec4f(r, g, b, a));
}
