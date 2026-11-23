//! A render's record as Render Info lists it, with the working version's
//! value under each row that differs.

use std::path::{Path, PathBuf};

use deptheos_core::Setting;

use super::{Compared, Working};
use crate::frames::folder::{Shot, is_raw_file};
use crate::frames::names::file_label;
use crate::library::history::Render;
use crate::library::stacks::Settings;
use crate::runs::stacker::{StackerConfig, align_label, short};
use crate::words::explain::setting_name;
use crate::words::{SEPARATOR, Words, say};

/// Which part of a render's record a [`SetupSection`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetupPart {
    /// The method, then each of its sliders.
    Method,
    /// Align Frames, then the aligner and crop if it aligned.
    Alignment,
    /// The camera RAW decoder: only for a render of RAW frames.
    Raw,
    /// How many frames were included, their order, then each frame.
    Frames,
    /// The frames left out, and why, if any.
    LeftOut,
}

/// One section of a render's record: a title and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupSection {
    pub part: SetupPart,
    /// "Method"
    pub title: String,
    pub rows: Vec<SetupRow>,
}

/// One row: "Smoothness" and "4", with "Working version: 6" when it differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupRow {
    pub label: String,
    /// Empty for an included frame with nothing more to say.
    pub value: String,
    /// The working version's other value, worded whole ("Not in the working
    /// version"), if any ([`Self::differs`]).
    pub working: Option<String>,
    /// A Frames or Left out row's frame, for a thumbnail.
    pub file: Option<PathBuf>,
}

impl SetupRow {
    /// A row of `value` against `working` (`None` when not compared).
    fn new(label: String, value: String, working: Option<String>, words: &Words) -> Self {
        let working =
            working.filter(|w| *w != value).map(|w| say!(words, "render-setup-working", value = w));
        Self { label, value, working, file: None }
    }

    /// Whether the working version differs: the row shows the edited dot.
    pub fn differs(&self) -> bool {
        self.working.is_some()
    }
}

/// "Stacked Nov 14, 2026 at 10:42"; `date` as the front end formats it.
pub fn stacked_at(date: &str, words: &Words) -> String {
    say!(words, "render-stacked-at", date = date)
}

/// `render`'s record for Render Info: Method, Alignment, RAW, Frames and
/// Left out. A row says when `working` differs in what `compared` counts;
/// frames compare as [`Working::changes`] counts them.
pub fn render_setup(
    render: &Render,
    working: &Working,
    compared: Compared,
    words: &Words,
) -> Vec<SetupSection> {
    let on_off = |on: bool| if on { say!(words, "setup-on") } else { say!(words, "setup-off") };
    let settings = working.settings.as_ref();
    let made = Settings { config: render.config, options: render.options };
    // The working version's value of a setting, where it is compared.
    let theirs =
        |counts: bool, value: &dyn Fn(&Settings) -> String| settings.filter(|_| counts).map(value);

    let mut method = vec![SetupRow::new(
        say!(words, "setup-method"),
        made.config.kind().label(words),
        theirs(true, &|s| s.config.kind().label(words)),
        words,
    )];
    let same_method = settings.filter(|s| s.config.kind() == made.config.kind());
    let working_sliders = same_method.map(|s| sliders(&s.config));
    for (setting, value) in sliders(&made.config) {
        let working = working_sliders
            .as_ref()
            .and_then(|w| w.iter().find(|(s, _)| *s == setting))
            .map(|(_, v)| v.clone());
        method.push(SetupRow::new(setting_name(setting, words), value, working, words));
    }

    let options = made.options;
    let mut alignment = vec![SetupRow::new(
        say!(words, "setup-align-frames"),
        on_off(options.alignment.is_on()),
        theirs(compared.align, &|s| on_off(s.options.alignment.is_on())),
        words,
    )];
    // The aligner and crop only when both align.
    if let Some(mode) = options.alignment.mode() {
        let aligning = settings.filter(|s| s.options.alignment.is_on());
        let theirs = |counts: bool, value: &dyn Fn(&Settings) -> String| {
            aligning.filter(|_| counts).map(value)
        };
        let their_mode = |s: &Settings| s.options.alignment.mode().map(|m| align_label(m, words));
        alignment.push(SetupRow::new(
            say!(words, "setup-aligner"),
            align_label(mode, words),
            theirs(compared.align_mode, &|s| their_mode(s).unwrap_or_default()),
            words,
        ));
        alignment.push(SetupRow::new(
            say!(words, "setup-crop"),
            on_off(options.alignment.crops()),
            theirs(true, &|s| on_off(s.options.alignment.crops())),
            words,
        ));
    }

    let mut sections = vec![
        SetupSection { part: SetupPart::Method, title: say!(words, "setup-method"), rows: method },
        SetupSection {
            part: SetupPart::Alignment,
            title: say!(words, "setup-alignment"),
            rows: alignment,
        },
    ];
    if render.linear {
        let decoder = SetupRow::new(
            say!(words, "setup-decoder"),
            options.raw_backend.label().to_string(),
            theirs(compared.raw_backend, &|s| s.options.raw_backend.label().to_string()),
            words,
        );
        sections.push(SetupSection {
            part: SetupPart::Raw,
            title: say!(words, "setup-raw"),
            rows: vec![decoder],
        });
    }
    sections.push(SetupSection {
        part: SetupPart::Frames,
        title: say!(words, "setup-frames"),
        rows: frame_rows(render, working, compared, words),
    });
    if !render.left_out.is_empty() {
        let rows = render
            .left_out
            .iter()
            .map(|l| SetupRow {
                file: Some(l.path.clone()),
                ..SetupRow::new(file_label(&l.path), l.reason.clone(), None, words)
            })
            .collect();
        sections.push(SetupSection {
            part: SetupPart::LeftOut,
            title: say!(words, "setup-left-out"),
            rows,
        });
    }
    sections
}

/// The Frames section: the count, the order, then each frame with
/// "Excluded" and, for pairs, RAW or JPEG.
fn frame_rows(
    render: &Render,
    working: &Working,
    compared: Compared,
    words: &Words,
) -> Vec<SetupRow> {
    let included_of = |included: usize, total: usize| {
        say!(words, "setup-included-of", included = included.to_string(), total = total.to_string())
    };
    let listed = working.list.frames();
    let mut rows = vec![
        SetupRow::new(
            say!(words, "setup-included"),
            included_of(render.included.iter().filter(|i| **i).count(), render.frames.len()),
            Some(included_of(listed.iter().filter(|(_, i)| *i).count(), listed.len())),
            words,
        ),
        SetupRow::new(
            say!(words, "setup-order"),
            render.options.order.label(words),
            compared.order.then(|| working.list.order.label(words)),
            words,
        ),
    ];

    let shots = working.list.shots();
    for (path, &included) in render.frames.iter().zip(&render.included) {
        let shot = shots.iter().find(|s| s.contains(path));
        let left_out = render.left_out.iter().any(|l| &l.path == path);
        let pair = shot.filter(|s| s.is_pair()).map(|_| is_raw_file(path));
        let value = frame_value(!included, pair, words);
        let mut row = SetupRow::new(file_label(path), value.clone(), None, words);
        row.file = Some(path.clone());
        match shot {
            None => {
                row.working = Some(say!(words, "render-setup-not-in-working"));
            }
            Some(shot) => {
                // A frame the stack left out counts the same either way.
                let theirs_included =
                    if left_out { included } else { working.list.is_included(shot) };
                let theirs_pair = pair.map(|_| is_raw_file(their_file(shot, working)));
                let theirs = frame_value(!theirs_included, theirs_pair, words);
                if theirs != value {
                    let theirs = if theirs.is_empty() {
                        say!(words, "setup-frame-included")
                    } else {
                        theirs
                    };
                    row.working = Some(say!(words, "render-setup-working", value = theirs));
                }
            }
        }
        rows.push(row);
    }
    rows
}

/// The file of `shot` the working version stacks.
fn their_file<'s>(shot: &'s Shot, working: &Working) -> &'s Path {
    shot.file(working.list.source)
}

/// A frame row's value: "Excluded", "RAW" or "JPEG" (`pair`: the RAW was
/// stacked), both, or nothing.
fn frame_value(excluded: bool, pair: Option<bool>, words: &Words) -> String {
    let excluded = excluded.then(|| say!(words, "setup-excluded"));
    let pair =
        pair.map(
            |raw| {
                if raw { say!(words, "setup-pair-raw") } else { say!(words, "setup-pair-jpeg") }
            },
        );
    excluded.into_iter().chain(pair).collect::<Vec<_>>().join(SEPARATOR)
}

/// The sliders of `config`'s method, valued as they show.
fn sliders(config: &StackerConfig) -> Vec<(Setting, String)> {
    match config {
        StackerConfig::Pyramid(c) => vec![(Setting::Smoothness, short(c.smoothness))],
        StackerConfig::Depth(c) => vec![
            (Setting::Radius, short(c.radius)),
            (Setting::Smoothing, short(c.smoothing)),
            (Setting::BlendRange, short(c.blend_range)),
        ],
        StackerConfig::Wavelet(c) => vec![
            (Setting::Denoise, short(c.denoise)),
            (Setting::Consistency, c.consistency.to_string()),
        ],
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::library::history::LeftOutFrame;
    use crate::library::ids::{RenderId, StackId};
    use crate::{DepthConfig, PyramidConfig, SessionOptions};

    /// Render #`number` of one stack: Pyramid at Smoothness 4, five JPEG
    /// frames of /a/Bee, all included.
    pub(in crate::library::stacks) fn render(number: u32) -> Render {
        let frames: Vec<PathBuf> =
            (1..=5).map(|i| PathBuf::from(format!("/a/Bee/{i}.jpg"))).collect();
        Render {
            id: RenderId::parse(&format!("{number:032x}")).unwrap(),
            created: SystemTime::UNIX_EPOCH,
            name: None,
            stack: StackId::parse(&"0".repeat(32)).unwrap(),
            number,
            config: StackerConfig::Pyramid(PyramidConfig { smoothness: 4.0 }),
            options: SessionOptions::default(),
            included: vec![true; frames.len()],
            frames,
            batch: None,
            picked: false,
            width: 4,
            height: 3,
            linear: false,
            left_out: Vec::new(),
        }
    }

    /// `render`'s working version, based on it, with `frames` shots
    /// excluded and `settings` (up to 3) settings changed.
    pub(in crate::library::stacks) fn edit(
        render: &Render,
        frames: usize,
        settings: usize,
    ) -> Working {
        let mut working = Working::of_render(render);
        for shot in working.list.shots().into_iter().take(frames) {
            working.list.set_included(&shot, false);
        }
        let options = &mut working.settings.as_mut().unwrap().options;
        let changes: [&dyn Fn(&mut SessionOptions); 3] = [
            &|o| o.alignment = o.alignment.crop_turned_over(),
            &|o| o.alignment = o.alignment.other_aligner(),
            &|o| {
                o.raw_backend = match o.raw_backend {
                    crate::RawBackend::LibRaw => crate::RawBackend::AdobeDng,
                    _ => crate::RawBackend::LibRaw,
                }
            },
        ];
        for change in &changes[..settings] {
            change(options);
        }
        working
    }

    fn words() -> Words {
        Words::english(crate::Platform::Desktop)
    }

    /// The rows as "label: value", with " → working" where it differs.
    fn said(sections: &[SetupSection]) -> Vec<(SetupPart, String, Vec<String>)> {
        sections
            .iter()
            .map(|s| {
                let rows = s
                    .rows
                    .iter()
                    .map(|r| match &r.working {
                        Some(w) => format!("{}: {} → {w}", r.label, r.value),
                        None => format!("{}: {}", r.label, r.value),
                    })
                    .collect();
                (s.part, s.title.clone(), rows)
            })
            .collect()
    }

    #[test]
    fn a_render_its_working_version_equals_lists_no_differences() {
        let render = render(3);
        let sections = render_setup(&render, &Working::of_render(&render), Compared::ALL, &words());
        let rows = |part| said(&sections).into_iter().find(|(p, ..)| *p == part).unwrap().2;
        assert_eq!(
            sections.iter().map(|s| (s.part, s.title.as_str())).collect::<Vec<_>>(),
            [
                (SetupPart::Method, "Method"),
                (SetupPart::Alignment, "Alignment"),
                (SetupPart::Frames, "Frames")
            ]
        );
        assert_eq!(rows(SetupPart::Method), ["Method: Pyramid", "Smoothness: 4"]);
        assert_eq!(
            rows(SetupPart::Alignment),
            [
                "Align frames: On",
                "Aligner: ECC (affine)",
                "Crop to the area every frame covers: On"
            ]
        );
        assert_eq!(
            rows(SetupPart::Frames),
            [
                "Included: 5 of 5",
                "Order: Name",
                "1.jpg: ",
                "2.jpg: ",
                "3.jpg: ",
                "4.jpg: ",
                "5.jpg: "
            ]
        );
        let frames = &sections[2].rows;
        assert_eq!(frames[2].file.as_deref(), Some(Path::new("/a/Bee/1.jpg")));
        assert!(sections.iter().flat_map(|s| &s.rows).all(|r| !r.differs()));
    }

    /// A render that didn't align lists no aligner or crop.
    #[test]
    fn a_render_not_aligned_lists_align_frames_alone() {
        let mut render = render(3);
        render.options.alignment = render.options.alignment.turned_over();
        let alignment = |working: &Working| {
            let sections = render_setup(&render, working, Compared::ALL, &words());
            said(&sections).into_iter().find(|(p, ..)| *p == SetupPart::Alignment).unwrap().2
        };
        assert_eq!(alignment(&Working::of_render(&render)), ["Align frames: Off"]);
        let mut working = Working::of_render(&render);
        working.settings.as_mut().unwrap().options.alignment = crate::Alignment::default();
        assert_eq!(alignment(&working), ["Align frames: Off → Working version: On"]);
    }

    /// The working version's values under the rows that differ: sliders
    /// only for the same method, frames by inclusion and RAW or JPEG, and
    /// the frames it doesn't list.
    #[test]
    fn rows_the_working_version_has_otherwise_say_its_value() {
        let w = words();
        let raw = |i: u32| PathBuf::from(format!("/a/Bee/{i}.CR3"));
        let jpeg = |i: u32| PathBuf::from(format!("/a/Bee/{i}.jpg"));
        let mut render = render(3);
        render.config =
            StackerConfig::Depth(DepthConfig { radius: 8.0, smoothing: 2.5, blend_range: 3.0 });
        render.linear = true;
        render.frames = (1..=4).map(raw).collect();
        render.included = vec![true, true, false, true];
        render.left_out =
            vec![LeftOutFrame { path: raw(4), reason: "couldn't be read as a RAW file".into() }];

        // The working version: #3's, with RAW + JPEG pairs stacking the
        // JPEG, 1 excluded, 3 included, 2 removed, Radius 6 and no crop.
        let mut working = Working::of_render(&render);
        working.list.files = vec![raw(1), jpeg(1), raw(3), jpeg(3), raw(4), jpeg(4)];
        working.list.source = crate::frames::folder::Source::Standard;
        working.list.excluded = [raw(1)].into();
        let settings = working.settings.as_mut().unwrap();
        settings.config =
            StackerConfig::Depth(DepthConfig { radius: 6.0, smoothing: 2.5, blend_range: 3.0 });
        settings.options.alignment = settings.options.alignment.crop_turned_over();

        let sections = render_setup(&render, &working, Compared::desktop(true), &w);
        let said = said(&sections);
        assert_eq!(
            said.iter().map(|(p, t, _)| (*p, t.as_str())).collect::<Vec<_>>(),
            [
                (SetupPart::Method, "Method"),
                (SetupPart::Alignment, "Alignment"),
                (SetupPart::Raw, "RAW"),
                (SetupPart::Frames, "Frames"),
                (SetupPart::LeftOut, "Left out"),
            ]
        );
        assert_eq!(
            said[0].2,
            ["Method: Depth", "Radius: 8 → Working version: 6", "Smoothing: 2.5", "Blend Range: 3"]
        );
        // The desktop doesn't offer Align Frames, so it never differs there.
        assert_eq!(
            said[1].2,
            [
                "Align frames: On",
                "Aligner: ECC (affine)",
                "Crop to the area every frame covers: On → Working version: Off"
            ]
        );
        assert_eq!(said[2].2, ["RAW decoder: LibRaw"]);
        assert_eq!(
            said[3].2,
            [
                "Included: 3 of 4 → Working version: 2 of 3",
                "Order: Name",
                "1.CR3: RAW → Working version: Excluded · JPEG",
                "2.CR3:  → Not in the working version",
                "3.CR3: Excluded · RAW → Working version: JPEG",
                "4.CR3: RAW → Working version: JPEG",
            ]
        );
        assert_eq!(said[4].2, ["4.CR3: couldn't be read as a RAW file"]);

        // Another method differs; its sliders aren't compared.
        let settings = working.settings.as_mut().unwrap();
        settings.config = StackerConfig::Pyramid(PyramidConfig { smoothness: 4.0 });
        let sections = render_setup(&render, &working, Compared::desktop(true), &w);
        assert_eq!(
            super::tests::said(&sections)[0].2,
            [
                "Method: Depth → Working version: Pyramid",
                "Radius: 8",
                "Smoothing: 2.5",
                "Blend Range: 3"
            ]
        );
    }

    #[test]
    fn the_stacked_line_takes_the_date_as_given() {
        assert_eq!(stacked_at("Nov 14, 2026 at 10:42", &words()), "Stacked Nov 14, 2026 at 10:42");
    }
}
