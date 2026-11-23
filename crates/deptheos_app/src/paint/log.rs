//! The tile codec: a painted tile or an undo snapshot as the bytes a row
//! of the paint file or the undo spill holds (`store.rs`), a codec byte,
//! then byte planes of left deltas compressed by zstd at level 1.

use std::io;

use super::tile::{TILE, TILE_PIXELS, TileData};

/// Codec version, the first byte of every tile.
const CODEC: u8 = 1;
const RAW_LEN: usize = TileData::BYTES;
const ZSTD_LEVEL: i32 = 1;

/// Compress a tile. Before zstd, the bytes are planes of left deltas
/// (each pixel minus the one before it in its row): for each RGB channel
/// the low bytes then the high bytes, then coverage, then source.
pub(crate) fn encode(tile: &TileData) -> Vec<u8> {
    let mut raw = Vec::with_capacity(RAW_LEN);
    let row = TILE as usize;
    for ch in 0..3 {
        let deltas: Vec<u16> = (0..TILE_PIXELS)
            .map(|i| {
                let v = tile.rgb[3 * i + ch];
                if i % row == 0 { v } else { v.wrapping_sub(tile.rgb[3 * (i - 1) + ch]) }
            })
            .collect();
        raw.extend(deltas.iter().map(|d| *d as u8));
        raw.extend(deltas.iter().map(|d| (d >> 8) as u8));
    }
    for plane in [&tile.coverage, &tile.source] {
        raw.extend(
            (0..TILE_PIXELS)
                .map(|i| if i % row == 0 { plane[i] } else { plane[i].wrapping_sub(plane[i - 1]) }),
        );
    }
    let mut out = vec![CODEC];
    out.extend(zstd::bulk::compress(&raw, ZSTD_LEVEL).expect("zstd compresses in memory"));
    out
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// Decompress a tile [`encode`] wrote.
pub(crate) fn decode(bytes: &[u8]) -> io::Result<TileData> {
    let (&codec, data) = bytes.split_first().ok_or_else(|| invalid("empty blob"))?;
    if codec != CODEC {
        return Err(invalid("unknown tile codec"));
    }
    // A blob that does not decompress is damaged, not unreachable.
    let raw = zstd::bulk::decompress(data, RAW_LEN)
        .map_err(|e| invalid(&format!("the tile does not decompress: {e}")))?;
    if raw.len() != RAW_LEN {
        return Err(invalid("truncated tile"));
    }
    let row = TILE as usize;
    let mut rgb = vec![0u16; 3 * TILE_PIXELS].into_boxed_slice();
    for ch in 0..3 {
        let (lo, hi) = raw[2 * ch * TILE_PIXELS..][..2 * TILE_PIXELS].split_at(TILE_PIXELS);
        for i in 0..TILE_PIXELS {
            let d = u16::from(lo[i]) | u16::from(hi[i]) << 8;
            rgb[3 * i + ch] = if i % row == 0 { d } else { rgb[3 * (i - 1) + ch].wrapping_add(d) };
        }
    }
    let plane = |at: usize| {
        let deltas = &raw[at..at + TILE_PIXELS];
        let mut out = vec![0u8; TILE_PIXELS].into_boxed_slice();
        for i in 0..TILE_PIXELS {
            out[i] = if i % row == 0 { deltas[i] } else { out[i - 1].wrapping_add(deltas[i]) };
        }
        out
    };
    Ok(TileData { rgb, coverage: plane(6 * TILE_PIXELS), source: plane(7 * TILE_PIXELS) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_round_trip_through_the_codec() {
        let mut tile = TileData::blank();
        for (i, v) in tile.rgb.iter_mut().enumerate() {
            *v = (i as u32).wrapping_mul(2654435761).rotate_right(16) as u16;
        }
        tile.coverage.iter_mut().enumerate().for_each(|(i, c)| *c = (i * 7) as u8);
        tile.source.iter_mut().enumerate().for_each(|(i, s)| *s = (i / 1000) as u8);
        let bytes = encode(&tile);
        assert!(bytes.len() <= 2 * RAW_LEN);
        assert!(decode(&bytes).unwrap() == tile);
        // A smooth tile compresses well.
        let flat = TileData::blank();
        assert!(encode(&flat).len() < 1000);
        // Garbage is an error, not a panic.
        assert!(decode(&[CODEC, 1, 2, 3]).is_err());
        assert!(decode(&[]).is_err());
    }
}
