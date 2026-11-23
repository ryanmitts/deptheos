//! What an export says about itself: the reference frame's camera, lens,
//! exposure and capture time (no GPS), that Deptheos made it, and its
//! source files, by name only, in XMP `xmpMM:Ingredients` beside a few
//! `deptheos:` properties. Exif `UserComment` sums them up.

use std::path::Path;

use deptheos_core::io::IccProfile;
use deptheos_core::io::metadata::{ExportMetadata, Value, exif_now, xml_escape};

use crate::frames::capture::{
    SourceTags, TAG_DATE_TIME_ORIGINAL, TAG_OFFSET_TIME_ORIGINAL, TAG_SUB_SEC_TIME_ORIGINAL,
    read_source_tags,
};
use crate::runs::session::{CompletedStack, StackColor};

/// The `Software` and `xmp:CreatorTool` value.
pub(crate) fn software() -> String {
    format!("Deptheos {}", env!("CARGO_PKG_VERSION"))
}

/// The namespace of Deptheos's own XMP properties.
pub(crate) const XMP_NAMESPACE: &str = "https://deptheos.com/ns/stack/1.0/";

/// The most bytes of XMP an export carries, under a JPEG segment's 65502;
/// ingredients past it are left out.
const XMP_BUDGET: usize = 60_000;

/// The metadata to export `stack` with.
pub(crate) fn export_metadata(stack: &CompletedStack) -> ExportMetadata {
    StackTags::read(stack).metadata(None)
}

/// What a stack's export metadata is made from: the reference frame's
/// tags, read once, and the stack's facts; also each sequence frame's
/// ([`Self::metadata`]).
pub(crate) struct StackTags {
    tags: SourceTags,
    icc: Option<IccProfile>,
    /// Source file names in the order given.
    names: Vec<String>,
    reference: Option<String>,
    method: &'static str,
    /// EXIF `YYYY:MM:DD HH:MM:SS`, UTC.
    modified: String,
}

/// Which sequence frame an export is: `k` of `of`, of turntable stack
/// `stack` (from 1) if any.
#[derive(Clone, Copy)]
pub(crate) struct SequenceFrame {
    pub k: usize,
    pub of: usize,
    pub stack: Option<usize>,
}

impl StackTags {
    pub(crate) fn read(stack: &CompletedStack) -> Self {
        let reference = stack.geometry.reference().map(|i| stack.paths[i].as_path());
        Self {
            tags: reference.map(source_tags).unwrap_or_default(),
            icc: icc(stack),
            names: stack.paths.iter().map(|p| file_name(p)).collect(),
            reference: reference.map(file_name),
            method: stack.config.kind().english_name(),
            modified: exif_now(),
        }
    }

    /// The metadata for the stack, or for its sequence frame `frame`.
    pub(crate) fn metadata(&self, frame: Option<SequenceFrame>) -> ExportMetadata {
        let mut props =
            vec![("Method", self.method.to_string()), ("FrameCount", self.names.len().to_string())];
        if let Some(SequenceFrame { k, stack, .. }) = frame {
            props.push(("SequenceDepth", k.to_string()));
            props.extend(stack.map(|s| ("SequenceStack", s.to_string())));
        }
        props.extend(self.reference.clone().map(|r| ("ReferenceFrame", r)));
        let comment = match frame {
            Some(frame) => frame.comment(),
            None => self.comment(),
        };
        ExportMetadata {
            ifd0: self.tags.ifd0.clone(),
            exif: self.tags.exif.clone(),
            software: software(),
            comment: Some(comment),
            xmp: Some(xmp(&self.modified, &props, &self.names, XMP_BUDGET)),
            modified: self.modified.clone(),
            icc: self.icc.clone(),
        }
    }

    /// "Focus stack of 47 frames, P1010001.ORF to P1010047.ORF".
    fn comment(&self) -> String {
        match self.names.as_slice() {
            [] => "Focus stack".into(),
            [one] => format!("Focus stack of 1 frame, {one}"),
            [first, .., last] => {
                format!("Focus stack of {} frames, {first} to {last}", self.names.len())
            }
        }
    }
}

impl SequenceFrame {
    /// "Sequence frame: 12 of 66 frames stacked, turntable stack 3".
    fn comment(&self) -> String {
        let stack_note = self.stack.map(|s| format!(", turntable stack {s}")).unwrap_or_default();
        format!("Sequence frame: {} of {} frames stacked{stack_note}", self.k, self.of)
    }
}

/// The metadata for frame `index` of `stack` exported aligned
/// ([`crate::runs::aligned`]): that frame's tags (no GPS) and the frame it
/// was aligned to; its one ingredient is the frame.
pub(crate) fn aligned_frame_metadata(stack: &CompletedStack, index: usize) -> ExportMetadata {
    let frame = stack.paths[index].as_path();
    let tags = source_tags(frame);
    let reference = stack.geometry.reference().map(|i| file_name(&stack.paths[i]));
    let modified = exif_now();
    let count = stack.paths.len();
    let comment = match &reference {
        Some(reference) => {
            format!("Frame {} of {count} of a focus stack, aligned to {reference}", index + 1)
        }
        None => format!("Frame {} of {count} of a focus stack", index + 1),
    };
    let mut props =
        vec![("AlignedFrame", (index + 1).to_string()), ("FrameCount", count.to_string())];
    props.extend(reference.map(|r| ("ReferenceFrame", r)));
    ExportMetadata {
        ifd0: tags.ifd0,
        exif: tags.exif,
        software: software(),
        comment: Some(comment),
        xmp: Some(xmp(&modified, &props, &[file_name(frame)], XMP_BUDGET)),
        modified,
        icc: icc(stack),
    }
}

/// The tags to copy from `path`; none when unreadable.
fn source_tags(path: &Path) -> SourceTags {
    read_source_tags(path).unwrap_or_else(|e| {
        tracing::debug!("no tags from {}: {e}", path.display());
        SourceTags::default()
    })
}

/// The ICC profile an export of `stack` carries: its own when sRGB. A
/// linear stack renders to sRGB but for DNG.
fn icc(stack: &CompletedStack) -> Option<IccProfile> {
    match &stack.color {
        StackColor::Srgb { profile, .. } => profile.clone(),
        StackColor::Linear { .. } => None,
    }
}

/// Unlike [`crate::frames::names::file_label`], nothing when there is no
/// name: a folder path is not for metadata.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The XMP packet: creator tool, `modified` (EXIF, UTC), `props` as
/// `deptheos:` properties, and as many `ingredients` as fit in `budget`
/// bytes, noting any left out.
fn xmp(modified: &str, props: &[(&str, String)], ingredients: &[String], budget: usize) -> String {
    let mut head = format!(
        concat!(
            "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n",
            " <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
            "  <rdf:Description rdf:about=\"\"\n",
            "    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n",
            "    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"\n",
            "    xmlns:stRef=\"http://ns.adobe.com/xap/1.0/sType/ResourceRef#\"\n",
            "    xmlns:deptheos=\"{ns}\"\n",
            "    xmp:CreatorTool=\"{tool}\"\n",
            "    xmp:ModifyDate=\"{iso}Z\""
        ),
        ns = XMP_NAMESPACE,
        tool = xml_escape(&software()),
        iso = exif_to_iso(modified),
    );
    for (name, value) in props {
        head.push_str(&format!("\n    deptheos:{name}=\"{}\"", xml_escape(value)));
    }
    let tail = "  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";
    let list_open = "   <xmpMM:Ingredients>\n    <rdf:Bag>\n";
    let list_close = "    </rdf:Bag>\n   </xmpMM:Ingredients>\n";
    // Always counted, so the note fits once the list is cut; 2 is the `>\n`
    // closing the head.
    let truncated = "\n    deptheos:IngredientsTruncated=\"True\"";

    let mut items = String::new();
    let mut listed = 0;
    let fixed = head.len() + truncated.len() + 2 + list_open.len() + list_close.len();
    for name in ingredients {
        let item = format!(
            "     <rdf:li rdf:parseType=\"Resource\">\
             <stRef:filePath>{}</stRef:filePath></rdf:li>\n",
            xml_escape(name)
        );
        if fixed + items.len() + item.len() + tail.len() > budget {
            break;
        }
        items.push_str(&item);
        listed += 1;
    }
    if listed < ingredients.len() {
        head.push_str(truncated);
    }
    let mut xmp = head;
    xmp.push_str(">\n");
    if listed > 0 {
        xmp.push_str(list_open);
        xmp.push_str(&items);
        xmp.push_str(list_close);
    }
    xmp.push_str(tail);
    xmp
}

/// The reference frame's IFD0 and Exif tags, for a DNG export to write.
pub(crate) fn dng_fields(metadata: &ExportMetadata) -> Vec<deptheos_dng::DngField> {
    use deptheos_dng::{DngField, DngValue};
    metadata
        .ifd0
        .iter()
        .chain(&metadata.exif)
        .map(|f| {
            let value = match &f.value {
                Value::Ascii(s) => DngValue::Text(s.clone()),
                Value::Byte(v) | Value::Undefined(v) => {
                    DngValue::Integers(v.iter().map(|&b| u32::from(b)).collect())
                }
                Value::Short(v) => DngValue::Integers(v.iter().map(|&s| u32::from(s)).collect()),
                Value::Long(v) => DngValue::Integers(v.clone()),
                Value::Rational(v) => DngValue::Rationals(v.clone()),
                Value::SRational(v) => DngValue::SRationals(v.clone()),
            };
            DngField { tag: f.tag, value }
        })
        .collect()
}

/// The copied `DateTimeOriginal`, with fraction and offset if recorded, as
/// ISO 8601, or empty.
pub(crate) fn capture_time_iso(metadata: &ExportMetadata) -> String {
    let text = |tag: u16| {
        metadata.exif.iter().find(|f| f.tag == tag).and_then(|f| match &f.value {
            Value::Ascii(s) => Some(s.trim().to_string()),
            _ => None,
        })
    };
    let Some(time) = text(TAG_DATE_TIME_ORIGINAL).filter(|t| t.len() == 19) else {
        return String::new();
    };
    let mut iso = exif_to_iso(&time);
    if let Some(sub) = text(TAG_SUB_SEC_TIME_ORIGINAL)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        iso.push('.');
        iso.push_str(&sub);
    }
    if let Some(offset) = text(TAG_OFFSET_TIME_ORIGINAL).filter(|o| o.len() == 6) {
        iso.push_str(&offset);
    }
    iso
}

/// EXIF `YYYY:MM:DD HH:MM:SS` as ISO 8601, without a zone.
fn exif_to_iso(exif: &str) -> String {
    exif.replacen(':', "-", 2).replacen(' ', "T", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(names: &[String]) -> StackTags {
        StackTags {
            tags: SourceTags::default(),
            icc: None,
            names: names.to_vec(),
            reference: names.first().cloned(),
            method: "Pyramid",
            modified: "2026:10:31 09:00:00".into(),
        }
    }

    fn xmp_of(names: &[String]) -> String {
        info(names).metadata(None).xmp.unwrap()
    }

    #[test]
    fn comment_names_the_ends() {
        let names = |n: usize| (1..=n).map(|i| format!("P{i:07}.ORF")).collect::<Vec<_>>();
        assert_eq!(info(&[]).comment(), "Focus stack");
        assert_eq!(info(&names(1)).comment(), "Focus stack of 1 frame, P0000001.ORF");
        assert_eq!(
            info(&names(47)).comment(),
            "Focus stack of 47 frames, P0000001.ORF to P0000047.ORF"
        );
    }

    #[test]
    fn xmp_lists_every_frame_by_name() {
        let names = vec!["IMG_1.CR3".to_string(), "A&B <2>.CR3".into()];
        let xmp = xmp_of(&names);
        assert!(xmp.contains("<stRef:filePath>IMG_1.CR3</stRef:filePath>"));
        assert!(xmp.contains("<stRef:filePath>A&amp;B &lt;2&gt;.CR3</stRef:filePath>"));
        assert!(xmp.contains("deptheos:FrameCount=\"2\""));
        assert!(xmp.contains("deptheos:ReferenceFrame=\"IMG_1.CR3\""));
        assert!(xmp.contains("xmp:ModifyDate=\"2026-10-31T09:00:00Z\""));
        assert!(!xmp.contains("Truncated"));
    }

    #[test]
    fn huge_stacks_are_cut_to_the_budget() {
        let names: Vec<String> = (0..2000).map(|i| format!("frame_{i:05}.NEF")).collect();
        let xmp = xmp_of(&names);
        assert!(xmp.len() <= XMP_BUDGET, "{} bytes", xmp.len());
        assert!(xmp.contains("deptheos:IngredientsTruncated=\"True\""));
        assert!(xmp.contains("deptheos:FrameCount=\"2000\""));
        assert!(xmp.contains("frame_00000.NEF"));
        assert!(xmp.ends_with("<?xpacket end=\"w\"?>"));
    }

    /// A sequence frame says which it is, within the budget.
    #[test]
    fn a_sequence_frame_says_which_it_is() {
        let names: Vec<String> = (0..2000).map(|i| format!("frame_{i:05}.NEF")).collect();
        let frame = SequenceFrame { k: 12, of: 66, stack: Some(3) };
        let tags = info(&names).metadata(Some(frame));
        let xmp = tags.xmp.unwrap();
        assert!(xmp.contains(concat!(
            "deptheos:FrameCount=\"2000\"\n",
            "    deptheos:SequenceDepth=\"12\"\n",
            "    deptheos:SequenceStack=\"3\"\n",
            "    deptheos:ReferenceFrame=\"frame_00000.NEF\""
        )));
        assert!(xmp.len() <= XMP_BUDGET, "{} bytes", xmp.len());
        assert_eq!(
            tags.comment.unwrap(),
            "Sequence frame: 12 of 66 frames stacked, turntable stack 3"
        );
        let frame = SequenceFrame { stack: None, ..frame };
        let xmp = info(&names).metadata(Some(frame)).xmp.unwrap();
        assert!(xmp.contains("deptheos:SequenceDepth=\"12\"") && !xmp.contains("SequenceStack"));
    }

    #[test]
    fn an_aligned_frame_names_itself_and_its_reference() {
        let paths = ["d/IMG_1.jpg", "d/A&B.jpg", "d/IMG_3.jpg"].map(std::path::PathBuf::from);
        let mut stack = CompletedStack::srgb(1, 1, vec![0; 3], paths.to_vec());
        stack.geometry.reversed = true;
        let tags = aligned_frame_metadata(&stack, 1);
        assert_eq!(
            tags.comment.as_deref(),
            Some("Frame 2 of 3 of a focus stack, aligned to IMG_3.jpg")
        );
        let xmp = tags.xmp.unwrap();
        assert!(xmp.contains("<stRef:filePath>A&amp;B.jpg</stRef:filePath>"), "{xmp}");
        assert!(!xmp.contains("IMG_1.jpg") && !xmp.contains("d/"), "{xmp}");
        assert!(xmp.contains("deptheos:AlignedFrame=\"2\""));
        assert!(xmp.contains("deptheos:ReferenceFrame=\"IMG_3.jpg\""));
        assert!(xmp.ends_with("<?xpacket end=\"w\"?>"));
    }

    #[test]
    fn capture_time_is_iso() {
        use deptheos_core::io::metadata::Field;
        let mut metadata = ExportMetadata::default();
        assert_eq!(capture_time_iso(&metadata), "");
        let time = Value::Ascii("2025:03:30 12:43:49".into());
        metadata.exif = vec![Field::new(TAG_DATE_TIME_ORIGINAL, time)];
        assert_eq!(capture_time_iso(&metadata), "2025-03-30T12:43:49");
        metadata.exif.push(Field::new(TAG_SUB_SEC_TIME_ORIGINAL, Value::Ascii("07".into())));
        metadata.exif.push(Field::new(TAG_OFFSET_TIME_ORIGINAL, Value::Ascii("-08:00".into())));
        assert_eq!(capture_time_iso(&metadata), "2025-03-30T12:43:49.07-08:00");
    }

    /// A DNG export gets every copied IFD0 and Exif tag as the frame has it.
    #[test]
    fn dng_fields_are_the_copied_tags() {
        use deptheos_core::io::metadata::{Field, TAG_ARTIST, TAG_MODEL};
        use deptheos_dng::{DngField, DngValue};
        let metadata = ExportMetadata {
            ifd0: vec![
                Field::new(TAG_MODEL, Value::Ascii("Canon EOS R5m2".into())),
                Field::new(TAG_ARTIST, Value::Ascii("A. Photographer".into())),
            ],
            exif: vec![
                Field::new(0x8827, Value::Short(vec![100])),
                Field::new(0x9204, Value::SRational(vec![[-1, 3]])),
                Field::new(0xA432, Value::Rational(vec![[100, 1], [100, 1], [0, 1], [0, 1]])),
            ],
            ..Default::default()
        };
        assert_eq!(
            dng_fields(&metadata),
            vec![
                DngField { tag: TAG_MODEL, value: DngValue::Text("Canon EOS R5m2".into()) },
                DngField { tag: TAG_ARTIST, value: DngValue::Text("A. Photographer".into()) },
                DngField { tag: 0x8827, value: DngValue::Integers(vec![100]) },
                DngField { tag: 0x9204, value: DngValue::SRationals(vec![[-1, 3]]) },
                DngField {
                    tag: 0xA432,
                    value: DngValue::Rationals(vec![[100, 1], [100, 1], [0, 1], [0, 1]]),
                },
            ]
        );
    }

    #[test]
    fn reference_is_the_first_frame_stacked() {
        use crate::runs::session::{LeftOut, StackGeometry};
        let geometry = |reversed, left_out: &[usize]| StackGeometry {
            transforms: vec![None; 4],
            crop: None,
            reversed,
            left_out: left_out
                .iter()
                .map(|&index| LeftOut { index, reason: "unreadable".into() })
                .collect(),
        };
        assert_eq!(geometry(false, &[]).reference(), Some(0));
        assert_eq!(geometry(true, &[]).reference(), Some(3));
        // A left-out first frame: the next one stacked is the reference.
        assert_eq!(geometry(false, &[0, 1]).reference(), Some(2));
        assert_eq!(geometry(true, &[3]).reference(), Some(2));
        assert_eq!(geometry(true, &[3]).stacked_order(), [2, 1, 0]);
        assert_eq!(geometry(false, &[0, 1, 2, 3]).reference(), None);
    }
}
