//! Checks of an English message file against its code (ADR-161): every
//! message and term used, every id defined, arguments named in comments,
//! and every message rendering whole as the snapshot records. Run by
//! `catalog.rs` and, through `test-support`, the desktop.
//!
//! To accept new wording, run the tests with `DEPTHEOS_UPDATE_SNAPSHOT=1`
//! and read the snapshot's diff.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use fluent::FluentValue;
use i18n_embed::fluent::FluentLanguageLoader;

/// One message file and the crate that words with it.
pub struct Catalog<'a> {
    /// The English file's text.
    pub english: &'a str,
    /// The crate's `src` folder: every `say!` and `fl!` in it counts.
    pub code: PathBuf,
    /// The English messages, loaded with no direction marks.
    pub loader: &'a FluentLanguageLoader,
    /// The snapshot of every message rendered.
    pub snapshot: PathBuf,
}

/// One message of the file, as written.
struct Message {
    id: String,
    /// Its text, continuation lines included.
    body: String,
    /// The comment lines directly above it (`#`, not a group's `##`).
    comment: String,
}

impl Message {
    /// The `$name`s it uses.
    fn variables(&self) -> BTreeSet<String> {
        names_after(&self.body, "$")
    }

    /// The ones a plural (or other) variant is chosen by: `{ $count ->`.
    fn selectors(&self) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for name in self.variables() {
            if self.body.contains(&format!("${name} ->")) {
                found.insert(name);
            }
        }
        found
    }
}

/// The identifiers after each `mark` in `text`.
fn names_after(text: &str, mark: &str) -> BTreeSet<String> {
    text.split(mark)
        .skip(1)
        .map(|rest| rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect())
        .filter(|name: &String| !name.is_empty())
        .collect()
}

/// The messages of `source`, terms aside. A message continues over lines
/// starting with a space or `}`, and blank lines before one.
fn messages(source: &str) -> Vec<Message> {
    let lines: Vec<&str> = source.lines().collect();
    let continues = |at: usize| {
        let next = lines[at..].iter().find(|l| !l.is_empty());
        next.is_some_and(|l| l.starts_with(' ') || l.starts_with('}'))
    };
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((id, first)) = line.split_once(" =") else { continue };
        let first = first.strip_prefix(' ').unwrap_or(first);
        let is_id = id.starts_with(|c: char| c.is_ascii_lowercase())
            && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !is_id {
            continue;
        }
        let mut body = first.to_string();
        for (at, next) in lines.iter().enumerate().skip(i + 1) {
            if !continues(at) {
                break;
            }
            body.push('\n');
            body.push_str(next);
        }
        let mut comment = Vec::new();
        for above in lines[..i].iter().rev() {
            match above.strip_prefix("# ").or_else(|| above.strip_prefix('#')) {
                Some(text) if !above.starts_with("##") => comment.push(text),
                _ => break,
            }
        }
        comment.reverse();
        found.push(Message { id: id.to_string(), body, comment: comment.join("\n") });
    }
    found
}

/// The ids in each `say!(…)` and `fl!` under `dir`, outside comments.
fn ids_used(dir: &Path) -> BTreeSet<String> {
    let mut sources = Vec::new();
    rust_files(dir, &mut sources);
    let mut used = BTreeSet::new();
    for text in sources {
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for macro_name in [concat!("say", "!("), concat!("fl", "!(")] {
            for call in code.split(macro_name).skip(1) {
                // The macro's own definition passes its arguments on.
                let Some((words, rest)) = call.split_once('"') else {
                    assert!(call.contains('$'), "a call names its message");
                    continue;
                };
                if words.contains('$') {
                    continue;
                }
                used.insert(rest.split('"').next().unwrap().to_string());
            }
        }
    }
    used
}

fn rust_files(dir: &Path, into: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(std::fs::read_to_string(&path).unwrap());
        }
    }
}

impl Catalog<'_> {
    /// Every message is used, every id defined, and more than `at_least`
    /// messages were parsed.
    pub fn assert_every_message_is_used_and_every_id_used_exists(&self, at_least: usize) {
        let defined: BTreeSet<String> = messages(self.english).into_iter().map(|m| m.id).collect();
        let used = ids_used(&self.code);
        let unused: Vec<&String> = defined.difference(&used).collect();
        let missing: Vec<&String> = used.difference(&defined).collect();
        assert!(unused.is_empty(), "messages no code uses: {unused:?}");
        assert!(missing.is_empty(), "ids the code uses that the file lacks: {missing:?}");
        assert!(defined.len() > at_least, "the file read as {} messages", defined.len());
    }

    /// Every term (`-name = …`) appears in some message.
    pub fn assert_every_term_is_used(&self) {
        let terms: Vec<&str> = self
            .english
            .lines()
            .filter_map(|l| l.strip_prefix('-')?.split_once(" ="))
            .map(|t| t.0)
            .collect();
        assert!(!terms.is_empty());
        for term in terms {
            assert!(self.english.contains(&format!("{{ -{term} }}")), "term -{term} is never used");
        }
    }

    /// Every `$name` a message uses is named in the comment above it.
    pub fn assert_every_argument_is_named_in_a_comment(&self) {
        let mut unnamed = Vec::new();
        for message in messages(self.english) {
            for name in message.variables() {
                if !message.comment.contains(&format!("${name}")) {
                    unnamed.push(format!("{}: ${name}", message.id));
                }
            }
        }
        assert!(unnamed.is_empty(), "arguments no comment above names: {unnamed:#?}");
    }

    /// Every message rendered with text arguments as `<name>` and plural
    /// selectors as 0, 1, 2 and 1000, each distinct rendering once.
    fn rendered(&self) -> String {
        let loader = self.loader;
        let mut out = String::new();
        for message in messages(self.english) {
            assert!(loader.has(&message.id), "{} isn't loaded", message.id);
            let selectors = message.selectors();
            let counts: &[usize] = if selectors.is_empty() { &[0] } else { &[0, 1, 2, 1000] };
            let mut seen = BTreeSet::new();
            for &n in counts {
                let mut args: HashMap<&str, FluentValue> = HashMap::new();
                let variables = message.variables();
                for name in &variables {
                    let value = match selectors.contains(name) {
                        true => FluentValue::from(n),
                        false => FluentValue::from(format!("<{name}>")),
                    };
                    args.insert(name.as_str(), value);
                }
                let text = loader.get_args_concrete(&message.id, args);
                // Fluent shows what it couldn't resolve as `{$name}` or `{-term}`.
                let missing = text.contains("{$") || text.contains("{-");
                assert!(!missing, "{}: an argument is missing: {text}", message.id);
                assert!(
                    !text.contains(['\u{2068}', '\u{2069}']),
                    "{}: direction marks in {text:?}",
                    message.id
                );
                if !seen.insert(text.clone()) {
                    continue;
                }
                let shown: Vec<String> = selectors.iter().map(|s| format!("{s}={n}")).collect();
                match shown.is_empty() {
                    true => out.push_str(&format!("{}\n", message.id)),
                    false => out.push_str(&format!("{} ({})\n", message.id, shown.join(", "))),
                }
                for line in text.lines() {
                    match line.is_empty() {
                        true => out.push('\n'),
                        false => out.push_str(&format!("    {line}\n")),
                    }
                }
            }
        }
        out
    }

    /// Every message renders whole and as the snapshot records, or with
    /// `DEPTHEOS_UPDATE_SNAPSHOT` writes it.
    pub fn assert_renders_as_the_snapshot_records(&self) {
        let text = self.rendered();
        let path = &self.snapshot;
        if std::env::var_os("DEPTHEOS_UPDATE_SNAPSHOT").is_some() {
            std::fs::write(path, &text).unwrap();
            return;
        }
        let recorded = std::fs::read_to_string(path).unwrap_or_default();
        if recorded != text {
            let differs = recorded
                .lines()
                .zip(text.lines())
                .position(|(a, b)| a != b)
                .unwrap_or(recorded.lines().count().min(text.lines().count()));
            panic!(
                "the messages render differently from {}, first at line {}: {:?}; write it \
                 again with DEPTHEOS_UPDATE_SNAPSHOT=1 if the change is meant",
                path.display(),
                differs + 1,
                text.lines().nth(differs)
            );
        }
    }
}
