//! `e2e-check`: the end-to-end checks on files exported outside a Rust
//! test, as iOS's are (`script/e2e-ios.sh`).
//!
//!     e2e-check stack SET FILE [--bits N] [--reference PNG] [--bless]
//!     e2e-check paint SET PLAIN PAINTED SOURCE_FRAME
//!     e2e-check notes SET EXPORTS --case NAME [--references DIR] [--bless]
//!     e2e-check manifest SET          write a set's manifest, once (each frame's size and SHA-256)
//!     e2e-check eight FROM TO         a 16-bit image as an 8-bit PNG, rounded, its profile kept (the sets' PNG-8)
//!     e2e-check fit FROM TO SIDE      a 16-bit TIFF shrunk to SIDE on its long side, 16 bits, tags and profile kept (the Simulator's sets)
//!     e2e-check cases                 the cases in e2e.toml
//!     e2e-check case NAME             one, as JSON with its set's frames, for the iOS app and its test
//!
//! `notes` reads the iOS test hook's `<file>.json` beside each export and
//! checks a plain one as a stack and a retouched one against the plain
//! TIFF before it. Exits 1 when a check fails.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use deptheos_app::library::export::ExportFormat;
use deptheos_e2e::cases::{Case, Config};
use deptheos_e2e::{Expect, Frames, Report, Set, check_paint, check_stack};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("cases") => return config_command(|config| Ok(list_cases(config))),
        Some("manifest") => {
            let written = args
                .get(1)
                .ok_or_else(|| "manifest SET".to_string())
                .and_then(|set| Set::open(Path::new(set))?.write_manifest());
            return match written {
                Ok(path) => {
                    println!("wrote {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("e2e-check: {e}");
                    ExitCode::from(2)
                }
            };
        }
        Some("eight") => {
            return match (args.get(1), args.get(2)) {
                (Some(from), Some(to)) => match eight(Path::new(from), Path::new(to)) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("e2e-check: {e}");
                        ExitCode::from(2)
                    }
                },
                _ => {
                    eprintln!("e2e-check: eight FROM TO");
                    ExitCode::from(2)
                }
            };
        }
        Some("fit") => {
            let side = args.get(3).and_then(|v| v.parse::<u32>().ok());
            let fitted = match (args.get(1), args.get(2), side) {
                (Some(from), Some(to), Some(side)) => fit(Path::new(from), Path::new(to), side),
                _ => Err("fit FROM TO SIDE".into()),
            };
            return match fitted {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("e2e-check: {e}");
                    ExitCode::from(2)
                }
            };
        }
        Some("case") => {
            let name = args.get(1).cloned().unwrap_or_default();
            return config_command(move |config| config.case(&name)?.for_ios(config));
        }
        _ => {}
    }
    match run(&args) {
        Ok(reports) => {
            for r in &reports {
                println!("{r}");
            }
            let failed = reports.iter().filter(|r| !r.passed()).count();
            println!("{} checked, {failed} failed", reports.len());
            if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => {
            eprintln!("e2e-check: {e}");
            ExitCode::from(2)
        }
    }
}

/// Print what `f` makes of e2e.toml (`config_path`).
fn config_command(f: impl FnOnce(&Config) -> Result<String, String>) -> ExitCode {
    match Config::load(&config_path()).and_then(|c| f(&c)) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("e2e-check: {e}");
            ExitCode::from(2)
        }
    }
}

/// `from` as an 8-bit PNG at `to`, rounded, with its ICC profile and EXIF
/// (`eXIf`) kept, so a set's PNG-8 frames match their 16-bit masters
/// (script/e2e-sets.sh).
fn eight(from: &Path, to: &Path) -> Result<(), String> {
    use image::ImageDecoder as _;
    let failed = |e: &dyn std::fmt::Display| format!("{}: {e}", from.display());
    let wrote = |e: &dyn std::fmt::Display| format!("{}: {e}", to.display());
    let mut reader = image::ImageReader::open(from)
        .map_err(|e| failed(&e))?
        .with_guessed_format()
        .map_err(|e| failed(&e))?;
    // A 45 MP 16-bit frame is past the reader's default limit.
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|e| failed(&e))?;
    let icc = decoder.icc_profile().map_err(|e| failed(&e))?;
    let image = image::DynamicImage::from_decoder(decoder).map_err(|e| failed(&e))?;
    let (w, h) = (image.width(), image.height());
    let rgb: Vec<u8> = image
        .to_rgb16()
        .into_raw()
        .into_iter()
        .map(|v| ((u32::from(v) * 255 + 32767) / 65535) as u8)
        .collect();
    let exif = exif_block(from)?;
    let file = std::fs::File::create(to).map_err(|e| wrote(&e))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| wrote(&e))?;
    if let Some(icc) = icc {
        // iCCP: a name, a NUL, compression method 0, the zlib stream.
        let mut chunk = b"ICC Profile\0\0".to_vec();
        chunk.extend(zlib(&icc));
        writer.write_chunk(png::chunk::iCCP, &chunk).map_err(|e| wrote(&e))?;
    }
    writer.write_chunk(png::chunk::eXIf, &exif).map_err(|e| wrote(&e))?;
    writer.write_image_data(&rgb).map_err(|e| wrote(&e))?;
    writer.finish().map_err(|e| wrote(&e))
}

/// The frame's primary EXIF as a standalone TIFF stream (a PNG's `eXIf`),
/// without its storage tags.
fn exif_block(from: &Path) -> Result<Vec<u8>, String> {
    use exif::{Field, In};
    let failed = |e: &dyn std::fmt::Display| format!("{}: {e}", from.display());
    let file = std::fs::File::open(from).map_err(|e| failed(&e))?;
    let tags = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .map_err(|e| failed(&e))?;
    let fields: Vec<Field> = tags
        .fields()
        .filter(|f| f.ifd_num == In::PRIMARY)
        .filter(|f| !(f.tag.context() == exif::Context::Tiff && STORAGE.contains(&f.tag.number())))
        .cloned()
        .collect();
    let mut writer = exif::experimental::Writer::new();
    for f in &fields {
        writer.push_field(f);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    writer.write(&mut out, true).map_err(|e| failed(&e))?;
    Ok(out.into_inner())
}

/// The TIFF tags that describe how the pixels are stored, never copied to
/// a file of other pixels.
const STORAGE: &[u16] = &[
    256, 257, 258, 259, 262, 273, 277, 278, 279, 284, 317, 322, 323, 324, 325, 330, 338, 339, 700,
    34675,
];

/// `bytes` as an uncompressed zlib stream, for an `iCCP` chunk.
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = bytes.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend(((b << 16) | a).to_be_bytes());
    out
}

/// The 16-bit TIFF at `from` shrunk (Lanczos) to `side` on its long side,
/// written to `to` as a 16-bit TIFF with its tags and ICC profile: a frame
/// the Simulator's GPU takes (ADR-110; sips would write 8 bits). Shrunk,
/// not cropped, so a handheld drift stays the same share of the frame.
fn fit(from: &Path, to: &Path, side: u32) -> Result<(), String> {
    use exif::{Field, In, Tag, Value};
    use image::ImageDecoder as _;
    let failed = |e: &dyn std::fmt::Display| format!("{}: {e}", from.display());
    let mut reader = image::ImageReader::open(from)
        .map_err(|e| failed(&e))?
        .with_guessed_format()
        .map_err(|e| failed(&e))?;
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|e| failed(&e))?;
    let icc = decoder.icc_profile().map_err(|e| failed(&e))?;
    let image = image::DynamicImage::from_decoder(decoder).map_err(|e| failed(&e))?;
    if !matches!(image, image::DynamicImage::ImageRgb16(_)) {
        return Err(failed(&"not a 16-bit RGB image"));
    }
    let (w, h) = (image.width(), image.height());
    if side >= w.max(h) {
        return Err(failed(&format!("{w} × {h} is no larger than {side}")));
    }
    let scale = f64::from(side) / f64::from(w.max(h));
    let width = (f64::from(w) * scale).round() as u32;
    let height = (f64::from(h) * scale).round() as u32;
    let rgb = image::imageops::resize(
        &image.to_rgb16(),
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    // One strip of about 1 MiB at a time, little-endian.
    let row_bytes = width as usize * 6;
    let rows_per_strip = (1 << 20) / row_bytes + 1;
    let bytes: Vec<u8> = rgb.as_raw().iter().flat_map(|v| v.to_le_bytes()).collect();
    let strips: Vec<&[u8]> = bytes.chunks(row_bytes * rows_per_strip).collect();

    // The frame's tags but for its storage ones.
    let file = std::fs::File::open(from).map_err(|e| failed(&e))?;
    let tags = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .map_err(|e| failed(&e))?;
    let mut fields: Vec<Field> = tags
        .fields()
        .filter(|f| f.ifd_num == In::PRIMARY)
        .filter(|f| !(f.tag.context() == exif::Context::Tiff && STORAGE.contains(&f.tag.number())))
        .cloned()
        .collect();
    let field = |tag: Tag, value: Value| Field { tag, ifd_num: In::PRIMARY, value };
    fields.extend([
        field(Tag::ImageWidth, Value::Long(vec![width])),
        field(Tag::ImageLength, Value::Long(vec![height])),
        field(Tag::BitsPerSample, Value::Short(vec![16, 16, 16])),
        field(Tag::Compression, Value::Short(vec![1])),
        field(Tag::PhotometricInterpretation, Value::Short(vec![2])),
        field(Tag::SamplesPerPixel, Value::Short(vec![3])),
        field(Tag::RowsPerStrip, Value::Long(vec![rows_per_strip as u32])),
        field(Tag::PlanarConfiguration, Value::Short(vec![1])),
    ]);
    if let Some(icc) = icc {
        fields.push(field(Tag(exif::Context::Tiff, 34675), Value::Undefined(icc, 0)));
    }
    let mut writer = exif::experimental::Writer::new();
    for f in &fields {
        writer.push_field(f);
    }
    writer.set_strips(&strips, In::PRIMARY);
    let mut out = std::io::Cursor::new(Vec::new());
    writer.write(&mut out, true).map_err(|e| format!("{}: {e}", to.display()))?;
    std::fs::write(to, out.into_inner()).map_err(|e| format!("{}: {e}", to.display()))
}

/// e2e.toml: `DEPTHEOS_E2E_CONFIG`, or the repository's.
fn config_path() -> PathBuf {
    std::env::var_os("DEPTHEOS_E2E_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(deptheos_e2e::cases::default_path)
}

fn list_cases(config: &Config) -> String {
    let mut lines = vec![format!("sets in {}", config.sets_dir().display())];
    for case in &config.cases {
        let settings: Vec<String> = [
            ("smoothness", case.smoothness),
            ("radius", case.radius),
            ("smoothing", case.smoothing),
            ("blend_range", case.blend_range),
            ("denoise", case.denoise),
        ]
        .iter()
        .filter_map(|(n, v)| v.map(|v| format!("{n} {v}")))
        .collect();
        let method = format!("{:?}", case.method).to_lowercase();
        let mut method = if settings.is_empty() {
            method
        } else {
            format!("{method} ({})", settings.join(", "))
        };
        if let Some(decoder) = case.decoder {
            let decoder = serde_json::to_value(decoder).unwrap_or_default();
            method = format!("{method}, decoder {}", decoder.as_str().unwrap_or_default());
        }
        let exports: Vec<String> = case.exports.iter().map(|e| String::from(*e)).collect();
        let on: Vec<String> = case.on.iter().map(|p| format!("{p:?}").to_lowercase()).collect();
        lines.push(format!(
            "{:<16} {:<9} {:<22} {:<34} exports {}{}  on {}",
            case.name,
            format!("{:?}", case.tier).to_lowercase(),
            case.set,
            method,
            exports.join(", "),
            if case.paint { ", then paints" } else { "" },
            on.join(", ")
        ));
    }
    lines.join("\n")
}

fn run(args: &[String]) -> Result<Vec<Report>, String> {
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
    };
    let plain: Vec<&String> = {
        let mut skip = false;
        args.iter()
            .filter(|a| {
                let keep = !skip && !a.starts_with("--");
                skip = matches!(a.as_str(), "--bits" | "--reference" | "--references" | "--case");
                keep
            })
            .collect()
    };
    let bless = flag("--bless");
    match plain.as_slice() {
        [cmd, set, file] if *cmd == "stack" => {
            let file = Path::new(file);
            let format = ExportFormat::of(file).ok_or("an export's extension")?;
            let bits = bits_given(args)?
                .unwrap_or(if format == ExportFormat::Jpeg { 8 } else { 16 });
            let set = Set::open(Path::new(set))?;
            let frames = Frames::read(&set)?;
            let reference = value("--reference");
            Ok(vec![check_stack(
                &set,
                &frames,
                file,
                &Expect { format, bits },
                reference.as_deref(),
                bless,
            )])
        }
        [cmd, set, plain, painted, source] if *cmd == "paint" => {
            let set = Set::open(Path::new(set))?;
            let frames = Frames::read(&set)?;
            Ok(vec![check_paint(&set, &frames, Path::new(plain), Path::new(painted), Path::new(source))])
        }
        [cmd, set, exports] if *cmd == "notes" => {
            let name = value("--case").ok_or("notes needs --case NAME")?;
            let config = Config::load(&config_path())?;
            let case = config.case(&name.to_string_lossy())?;
            notes(Path::new(set), Path::new(exports), value("--references").as_deref(), case, bless)
        }
        _ => Err("usage: e2e-check stack SET FILE [--bits N] [--reference PNG] [--bless] | \
                  paint SET PLAIN PAINTED SOURCE_FRAME | manifest SET | notes SET EXPORTS --case NAME [--references DIR] [--bless]"
            .into()),
    }
}

/// The bits `--bits` asks for, if given: 8 or 16, anything else refused.
fn bits_given(args: &[String]) -> Result<Option<u8>, String> {
    let Some(i) = args.iter().position(|a| a == "--bits") else { return Ok(None) };
    match args.get(i + 1).map(String::as_str) {
        Some("8") => Ok(Some(8)),
        Some("16") => Ok(Some(16)),
        Some(other) => Err(format!("--bits {other}: 8 or 16")),
        None => Err("--bits needs a value: 8 or 16".into()),
    }
}

/// The iOS test hook's note beside an export (`<file>.json`), e.g.
/// `{"file": "1 Spider.tif", "format": "tiff", "bits": 16, "paintedFrom": "spider-01.jpg"}`;
/// `paintedFrom` is null when not retouched.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Note {
    file: PathBuf,
    #[serde(deserialize_with = "format_named")]
    format: ExportFormat,
    bits: u8,
    painted_from: Option<String>,
}

fn notes(
    set: &Path,
    exports: &Path,
    references: Option<&Path>,
    case: &Case,
    bless: bool,
) -> Result<Vec<Report>, String> {
    // The plain exports in the case's order, then the TIFF the test adds
    // to paint against when the case asks for none.
    let mut expected =
        case.exports.iter().map(|e| e.bits(case.stack_bits)).chain(std::iter::once(8));
    let set = Set::open(set)?;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(exports)
        .map_err(|e| format!("{}: {e}", exports.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    // Named in the order exported ("1 …", "2 …").
    paths.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str()?.split(' ').next()?.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    if paths.is_empty() {
        return Err(format!("{}: no exports noted", exports.display()));
    }
    let frames = Frames::read(&set)?;
    let mut reports = Vec::new();
    let mut plain_tiff: Option<PathBuf> = None;
    for path in paths {
        let note = read_note(&path, exports)?;
        match &note.painted_from {
            None => {
                // By what it is ("ios-tiff-16.png"), not when it was made.
                let reference = references.map(|r| {
                    r.join(format!("ios-{:?}-{}.png", note.format, note.bits).to_lowercase())
                });
                let bits = expected.next().unwrap_or(8);
                reports.push(check_stack(
                    &set,
                    &frames,
                    &note.file,
                    &Expect { format: note.format, bits },
                    reference.as_deref(),
                    bless,
                ));
                if note.format == ExportFormat::Tiff {
                    plain_tiff = Some(note.file.clone());
                }
            }
            Some(frame) => {
                let plain =
                    plain_tiff.as_ref().ok_or("a retouched export with no plain TIFF before it")?;
                let source = set.frame_named(frame).ok_or_else(|| {
                    format!("painted from {frame}, which isn't in {}", set.dir.display())
                })?;
                reports.push(check_paint(&set, &frames, plain, &note.file, source));
            }
        }
    }
    Ok(reports)
}

/// The note in `path`, its file in `dir`.
fn read_note(path: &Path, dir: &Path) -> Result<Note, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let note: Note = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Note { file: dir.join(note.file), ..note })
}

/// A note's format, as the iOS hook names it (`"tiff"`).
fn format_named<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ExportFormat, D::Error> {
    let name = <String as serde::Deserialize>::deserialize(d)?;
    ExportFormat::ALL
        .into_iter()
        .find(|f| f.key() == name)
        .ok_or_else(|| serde::de::Error::custom(format!("no export format named {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_8_or_16_and_anything_else_is_refused() {
        let args = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(bits_given(&args("stack s f.tif")), Ok(None));
        assert_eq!(bits_given(&args("stack s f.tif --bits 8")), Ok(Some(8)));
        assert_eq!(bits_given(&args("stack s f.tif --bits 16 --bless")), Ok(Some(16)));
        for bad in ["12", "sixteen", "--bless"] {
            let line = format!("stack s f.tif --bits {bad}");
            assert_eq!(bits_given(&args(&line)), Err(format!("--bits {bad}: 8 or 16")));
        }
        assert_eq!(
            bits_given(&args("stack s f.tif --bits")),
            Err("--bits needs a value: 8 or 16".into())
        );
        // Refused before the set is read; `main` exits 2 on it.
        let refused = run(&args("stack /no/set f.tif --bits 12")).map(|_| ());
        assert_eq!(refused, Err("--bits 12: 8 or 16".into()));
    }

    #[test]
    fn a_note_reads_as_json_whatever_its_file_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.json");
        let text = r#"{"bits":16,"file":"1 A \"bee\", then a wasp.tif","format":"tiff","paintedFrom":"spider, 01.jpg"}"#;
        std::fs::write(&path, text).unwrap();
        let note = read_note(&path, dir.path()).unwrap();
        assert_eq!(note.file, dir.path().join("1 A \"bee\", then a wasp.tif"));
        assert_eq!(note.format, ExportFormat::Tiff);
        assert_eq!(note.bits, 16);
        assert_eq!(note.painted_from.as_deref(), Some("spider, 01.jpg"));

        std::fs::write(&path, r#"{"bits":8,"file":"2 b.jpg","format":"jpeg","paintedFrom":null}"#)
            .unwrap();
        let note = read_note(&path, dir.path()).unwrap();
        assert_eq!((note.format, note.bits, note.painted_from), (ExportFormat::Jpeg, 8, None));
    }
}
