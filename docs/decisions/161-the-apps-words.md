# ADR-161: One page of terms governs every string; English only, the shared and desktop sentences in Fluent, iOS's in a String Catalog, the command line's in code

## Status

Accepted.

## Context

User-facing text comes from the desktop app (`crates/deptheos_gpui`), the iOS app (`ios/DeptheosMobile`), the shared layer (`crates/deptheos_app`), the command line (`crates/deptheos_cli`), `stackrig` and the website (`web/`). Without one written specification they drift: *frame*, *image* and *photo* for one thing, *the Library* and *the history*, capitalization, unit spacing, "1 frames", and errors chained as "alignment failed: ECC alignment failed: …" with internal type names showing.

Strings scattered as literals can't be audited in one place, can't pick plurals properly, and would have to be rewritten at every call site for a second language.

## Decision

### The rulebook

- **`docs/terms-and-usage.md` is the rulebook for every user-facing string**, in the desktop app, the iOS app, `deptheos`, `stackrig` and the website:
  - section 2, the term for each concept and the words to avoid;
  - section 3, capitalization (Title Case and sentence case), punctuation, the ellipsis, the middle dot (` · `), numbers and units;
  - section 4, the error and notice pattern: a Lead with the user's object as its subject, the Reason, What to do, and technical Details kept in Copy Details and the log, never a chain of "X failed:" prefixes;
  - section 5, singular, plural and zero as whole phrases;
  - section 6, what must match between the desktop and iOS and what may follow each platform.
- **The website's vocabulary is the reference, and spelling is American throughout**, the apps and the website alike (*color*, *canceled*, *gray*).
- **A change to a term updates `docs/terms-and-usage.md` first**, then the catalogs and the website.

### Engine errors

- **The engine crates return typed errors** (`deptheos_core`, `deptheos_gpu`, `deptheos_aligner`, `deptheos_dng`, `deptheos_raw`) whose `Display` is technical, for the log.
- **`deptheos_app::words::explain` words every engine error once** for the desktop, iOS and the command line. An internal fault with no sentence of its own gets one fixed one: "Deptheos ran into an unexpected problem and stopped." (`fault` in the shared messages).

### Where the strings live

- **English (`en-US`) is the only language shipped.**
- **iOS: Xcode String Catalogs**, `ios/DeptheosMobile/Localizable.xcstrings`, and `InfoPlist.xcstrings` for the system's permission prompts. Model code uses `LocalizedStringResource`, counts use the catalog's plural variants, and numbers, byte sizes and durations use `FormatStyle`.
- **The shared layer: a Fluent file**, `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`, loaded through `i18n-embed` into a `deptheos_app::words::Words` that each front end makes once and passes down; nothing is global, so two values can word the same error in two languages and the functions that take one stay pure. English stands behind any message a language lacks. A sentence is one message with named arguments, worded through `say!` over `fl!`, which checks the message id and argument names against the English file at build time. Rust words the shared sentences (errors in `explain.rs`, batch lines, setting descriptions) once for all three front ends; the bridge's own sentences are in `words/ios.rs`.
- **iOS gets the engine's sentences in its own language**: it passes `Bundle.main.preferredLocalizations.first` (`AppWords.language`, the language iOS chose for the app from its localizations, so the engine's sentences match the String Catalog's) to `Engine::new`, and shows the sentences it gets back as they are.
- **The desktop: a second Fluent file**, `crates/deptheos_gpui/i18n/en-US/deptheos_gpui.ftl`, loaded for the same language into a `DesktopWords` (`deptheos_gpui/src/words.rs`), which holds the shared `Words` beside the desktop's own messages. The app keeps it as a GPUI global, and views word a message with `say!(cx, "id", …)`.
- **Numbers are formatted in Rust**, because `fluent-bundle` does not implement `NUMBER()`'s options: numbers, sizes, dimensions and durations are passed as finished text (`words/units.rs`), and a count that chooses a plural is passed as a number for `[one]` and `[other]`. Fluent's bidi isolation marks (U+2068, U+2069) around arguments are off (`set_use_isolating(false)`).
- **The command line and `stackrig` keep their own strings and `--help` in code, in English**; the engine errors the command line prints still come from `explain`.

### Checks

`fl!` does not fail on an omitted argument or an unused message, so the tests in both `deptheos_app` and `deptheos_gpui` (the checks in `crates/deptheos_app/src/words/check.rs`, run on each crate's own file by its `words/catalog.rs`, both through `just test-words`):

1. parse the `.ftl` file and every `.rs` source, and assert that every message id is used and every id used exists;
2. render every message and attribute with sample arguments across singular and plural forms, with no `FluentError` and no `{ $arg }` left in the text;
3. pin every rendered message in a snapshot, `src/words/english.snap` in each crate.

## Alternatives considered

### User-facing sentences in the engine errors' `Display`

It mixes the apps' wording into the engine crates, loses the technical detail the log needs, and can't be localized by message id and arguments.

### Engine errors worded separately in Swift and in Rust

Rust passing fine-grained error enums over the bridge for Swift to word in `Localizable.xcstrings`, while also wording them in `.ftl` for the desktop and the command line, would keep every error and batch sentence twice, in two formats that drift apart.

### The desktop's strings inline in the Rust source

They would lack the unused-message, missing-argument and snapshot checks, and need hand-written plural helpers. With both Rust crates in `.ftl` files every string the GUI shows is checked the same way.

## Consequences

- Every new message, label, menu item or error follows `docs/terms-and-usage.md`; a new engine error is mapped in `crates/deptheos_app/src/words/explain.rs`.
- Changing a shared or desktop string means its `.ftl` file and `just test-words` (with `DEPTHEOS_UPDATE_SNAPSHOT=1` when the change is meant, then reading the snapshot's diff).
- A translation will be two standard formats: `.ftl` for the Rust crates and `.xcstrings` for iOS.

## Code

- `docs/terms-and-usage.md`.
- `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`, `crates/deptheos_app/src/words.rs`, `crates/deptheos_app/src/words/explain.rs`, `crates/deptheos_app/src/words/units.rs`, `crates/deptheos_app/src/words/ios.rs`, `crates/deptheos_app/src/words/check.rs`, `crates/deptheos_app/src/words/catalog.rs`, `crates/deptheos_app/src/words/english.snap`.
- `crates/deptheos_gpui/i18n/en-US/deptheos_gpui.ftl`, `crates/deptheos_gpui/src/words.rs`, `crates/deptheos_gpui/src/words/catalog.rs`, `crates/deptheos_gpui/src/words/english.snap`, `crates/deptheos_gpui/src/notices.rs`.
- `ios/DeptheosMobile/Localizable.xcstrings`, `ios/DeptheosMobile/InfoPlist.xcstrings`, `ios/DeptheosMobile/Model/AppWords.swift`.
