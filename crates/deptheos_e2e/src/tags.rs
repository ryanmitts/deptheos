//! A DNG export's color tags against its frames'.

use deptheos_dng::RenderParams;

use crate::{Frames, Report, Set};

/// How far a DNG export's color science may be from its frame's, in each
/// of its numbers, and in its BaselineExposure (stops).
const TAGS: f32 = 1e-3;
const TAGS_EXPOSURE: f64 = 0.01;

/// A DNG export's color tags, as the SDK turns them into render
/// parameters, match one of its frames' (the stack's reference frame).
pub(crate) fn color_tags(
    report: &mut Report,
    export: Option<&RenderParams>,
    set: &Set,
    frames: &Frames,
) {
    let Some(export) = export else {
        report.check("color tags", false, "the DNG has no color science");
        return;
    };
    let compared: Vec<(usize, Vec<String>)> = frames
        .colors
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.as_ref().map(|c| (i, tag_differences(export, c))))
        .collect();
    let name = |i: usize| {
        set.frames[i].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    };
    match compared.iter().min_by_key(|(_, d)| d.len()) {
        None => report.check("color tags", false, "no RAW or DNG frames to compare with"),
        Some((i, d)) if d.is_empty() => report.check(
            "color tags",
            true,
            format!(
                "matrices, AsShotNeutral, BaselineExposure {:+.2}, profile tables{} and tone curve as {}'s",
                export.baseline_exposure,
                if export.hue_sat_map.is_some() || export.look_table.is_some() {
                    ""
                } else {
                    " (none)"
                },
                name(*i)
            ),
        ),
        Some((i, d)) => {
            report.check("color tags", false, format!("against {}: {}", name(*i), d.join("; ")))
        }
    }
}

/// Where `export`'s color science differs from `frame`'s.
pub(crate) fn tag_differences(export: &RenderParams, frame: &RenderParams) -> Vec<String> {
    let worst = |a: &[f32], b: &[f32]| -> f32 {
        if a.len() != b.len() {
            return f32::INFINITY;
        }
        a.iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
    };
    let mut out = Vec::new();
    let off = worst(&export.camera_to_prophoto, &frame.camera_to_prophoto);
    if off > TAGS {
        out.push(format!("its color matrices are off by {off:.4}"));
    }
    if worst(&export.camera_white, &frame.camera_white) > TAGS {
        out.push(format!(
            "AsShotNeutral {:?}, the frame's {:?}",
            export.camera_white, frame.camera_white
        ));
    }
    if (export.baseline_exposure - frame.baseline_exposure).abs() > TAGS_EXPOSURE {
        out.push(format!(
            "BaselineExposure {:+.2}, the frame's {:+.2}",
            export.baseline_exposure, frame.baseline_exposure
        ));
    }
    let tables = [
        ("HueSatMap", &export.hue_sat_map, &frame.hue_sat_map),
        ("LookTable", &export.look_table, &frame.look_table),
    ];
    for (name, a, b) in tables {
        let same = match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                (a.hue_divisions, a.sat_divisions, a.val_divisions, a.srgb_encoded)
                    == (b.hue_divisions, b.sat_divisions, b.val_divisions, b.srgb_encoded)
                    && worst(&a.data, &b.data) <= TAGS
            }
            _ => false,
        };
        if !same {
            let has = |t: &Option<_>| if t.is_some() { "one" } else { "none" };
            out.push(format!("the profile's {name}: {} here, {} in the frame", has(a), has(b)));
        }
    }
    let off = worst(&export.tone_curve, &frame.tone_curve);
    if off > TAGS {
        out.push(format!("its tone curve is off by {off:.4}"));
    }
    if (export.shadow_scale - frame.shadow_scale).abs() > 1e-3 {
        out.push(format!(
            "its shadow scale {:.3}, the frame's {:.3}",
            export.shadow_scale, frame.shadow_scale
        ));
    }
    out
}
