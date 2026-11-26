//! A reframed export (ADR-136): its size, its picture put back as stored
//! to be matched with the frames, and where its crop sits on them.

use deptheos_adjust::reframe::Size;
use deptheos_adjust::{Flip, Reframe, Turn};

use crate::read::Small;
use crate::{Frames, MIN_SIDE, Report};

fn turned(turn: Turn) -> bool {
    matches!(turn, Turn::Quarter | Turn::ThreeQuarters)
}

fn degrees(turn: Turn) -> u32 {
    match turn {
        Turn::None => 0,
        Turn::Quarter => 90,
        Turn::Half => 180,
        Turn::ThreeQuarters => 270,
    }
}

/// Whether an export of `export` pixels fits a reframe of a stack of
/// `frames`-sized frames: the crop's size, turned, or with no crop the
/// stack's (no larger than the frames, at least [`MIN_SIDE`] of them),
/// turned. With what it says.
pub(crate) fn size(frames: (u32, u32), export: (u32, u32), reframe: &Reframe) -> (bool, String) {
    let (fw, fh) = frames;
    let turn = match reframe.turn {
        Turn::None => String::new(),
        t => format!(", turned {}°", degrees(t)),
    };
    if let Some(crop) = reframe.crop {
        let (w, h) = if turned(reframe.turn) {
            (crop.height, crop.width)
        } else {
            (crop.width, crop.height)
        };
        let inside = u64::from(crop.x) + u64::from(crop.width) <= u64::from(fw)
            && u64::from(crop.y) + u64::from(crop.height) <= u64::from(fh);
        let detail = format!(
            "{} × {}; the crop {} × {} at ({}, {}){turn} is {w} × {h}",
            export.0, export.1, crop.width, crop.height, crop.x, crop.y
        );
        if !inside {
            return (false, format!("{detail}, which reaches past the frames' {fw} × {fh}"));
        }
        return (export == (w, h), detail);
    }
    let (w, h) = if turned(reframe.turn) { (export.1, export.0) } else { export };
    let (w, h, fwf, fhf) = (w as f32, h as f32, fw as f32, fh as f32);
    let ok = w <= fwf && h <= fhf && w >= MIN_SIDE * fwf && h >= MIN_SIDE * fhf;
    let detail = format!(
        "{:.1}% × {:.1}% of the frames' {fw} × {fh}{} (at least {:.0}% each way)",
        100.0 * w / fwf,
        100.0 * h / fhf,
        if turn.is_empty() { String::new() } else { format!(" upright{turn}") },
        100.0 * MIN_SIDE
    );
    (ok, detail)
}

/// The export's picture with its flip and turn undone, as the stack holds
/// it (the crop's window of it).
pub(crate) fn upright(small: &Small, reframe: &Reframe) -> Small {
    if reframe.flip == Flip::None && reframe.turn == Turn::None {
        return small.clone();
    }
    // A mirror and a turn undo themselves; a turn alone undoes by the
    // turn that makes a whole one.
    let undo = match reframe.flip {
        Flip::Horizontal => Reframe { crop: None, ..*reframe },
        Flip::None => Reframe {
            turn: match reframe.turn {
                Turn::Quarter => Turn::ThreeQuarters,
                Turn::ThreeQuarters => Turn::Quarter,
                t => t,
            },
            ..Reframe::NONE
        },
    };
    let size = Size { width: small.w as u32, height: small.h as u32 };
    let (px, size) = undo.apply(&small.px, size, 1).expect("the whole picture is inside itself");
    Small { w: size.width as usize, h: size.height as usize, px }
}

/// Where the upright export sits on the frames, `at` in their pixels;
/// with a crop, at the crop's corner of a stack the aligner cut by at
/// most `1 - MIN_SIDE` of each side, within two measuring pixels.
pub(crate) fn placed(report: &mut Report, frames: &Frames, at: (usize, usize), reframe: &Reframe) {
    let detail = format!("at ({}, {}) on the frames", at.0, at.1);
    let Some(crop) = reframe.crop else {
        report.check("placed", true, detail);
        return;
    };
    let slack = 2.0 * frames.k as f32;
    let fits = |at: usize, corner: u32, side: u32| {
        let (at, corner) = (at as f32, corner as f32);
        at + slack >= corner && at <= corner + (1.0 - MIN_SIDE) * side as f32 + slack
    };
    let ok = fits(at.0, crop.x, frames.width) && fits(at.1, crop.y, frames.height);
    report.check(
        "placed",
        ok,
        format!(
            "{detail}; the crop's corner ({}, {}) of a stack cut by at most {:.0}% a side",
            crop.x,
            crop.y,
            100.0 * (1.0 - MIN_SIDE)
        ),
    );
}
