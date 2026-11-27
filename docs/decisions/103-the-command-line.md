# ADR-103: The command line: a command per method, paths on stdout, refusals before any work, never over an input

## Status

Accepted.

## Context

`deptheos` (`crates/deptheos_cli`) runs the engine from a terminal: for shell scripts, batch workflows, and the website's benchmark and figure generators (ADR-160). It ships in every release package (ADR-102). A shell glob can pick up a previous output with the frames, and the tool must never write over an input photo when it does. A stack takes minutes, so a flag combination that is wrong or does nothing must be refused before any of it.

## Decision

- **Commands** (`crates/deptheos_cli/src/args.rs`): `pyramid`, `depth` and `wavelet` stack one bracket; `batch <method> <folders…>` stacks one bracket per folder or splits folders by capture time (ADR-146); `align` writes each frame aligned as a stack of them would be, all one size, into a folder (`align.rs`, `deptheos_app::runs::aligned`); `sequence grow|turntable` writes numbered frames for a video editor (ADR-147); `redraw` draws one saved render again, with its paint, through the path adjustments take, cut, turned and flipped by `--reframe`, `--turn` and `--flip`, under the adjustment sliders' flags (`--exposure`, `--contrast`, `--highlights`, `--shadows`, `--whites`, `--blacks`, `--clarity`, `--texture`, `--sharpening` with `--sharpen-radius`, `--sharpen-detail` and `--sharpen-masking`, `--vibrance`, `--saturation`, `--temperature` and `--tint`, the last two in kelvins on a camera RAW render and on the relative scale on a standard one, checked once the render is read) read in `--process`'s version, into `--space`: srgb or own, and on a camera RAW render display-p3, adobe-rgb or prophoto, the last in a 16-bit TIFF or PNG only (`redraw.rs`, ADR-135, ADR-136, ADR-137); `bench` times the methods and aligners. Every stack runs through `Engine::session`, and batch and turntable runs drive `batch::Machine` (ADR-101, ADR-144).
- **One set of flags, typed.** `-a/--align ecc|grid|none`, `-o/--output`, `--bit-depth 8|16` and the rest are the same on every command, each value a clap `ValueEnum` that maps onto the engine's type. `-j/--jobs` is the camera RAW conversion's parallelism.
- **A flag that would do nothing is a usage error** (`Checks` in `args.rs`): `--jpeg-quality` without JPEG output, `--bit-depth` without TIFF or PNG output, `--grid-*` without the grid aligner, `--dng-converter`, `--jobs` or `--dng-uncompressed` without `--raw-backend adobe-dng`, `redraw`'s `--process` without an adjustment, and its color flags on a render with no color. A slider's flag outside its range is refused. Each fails at once with exit status 2. Dependent options have no clap default, so `None` tells "not passed" from "passed with the default value".
- **Streams for scripts** (`ui.rs`): stdout holds only the paths of the files written, one per line (`bench` prints its table there instead); progress, warnings and errors go to stderr. `-q` silences progress, `-v` logs the engine's steps at debug level, and `RUST_LOG` overrides both.
- **Exit status** (`main.rs`, listed in `--help`): `0` when everything asked for was written; `1` when an error stopped the run or it asked for something refused (an output that is a frame, a folder with no frames); `2` for an unknown or conflicting option, or a batch or turntable that finished with stacks that failed or couldn't be written (it goes on without them), or a turntable whose check left a stack out of the sequence (`Ended::of`); `130` on Ctrl-C.
- **Ctrl-C** (`interrupt.rs`): the first press cancels the engine's root token, so the run stops at once (killing a DNG conversion it waits on, a codec mid-decode), keeps what it wrote (a sequence's manifest marked incomplete) and deletes its temporary files on the way out. A second press exits at once; the next run sweeps what that left (`TempStore` in `sequence/store.rs`).
- **Outputs checked before any frame is read** (`outputs.rs`). An output that is one of the input frames by file identity (`same_file`: through links, `..` and a case-insensitive file system), or two outputs that are one file, are refused, with no override. An output in a folder that doesn't exist, with an extension Deptheos doesn't write, or `.dng` from frames that aren't RAW, fails the same way. A sweep's `-o` must be a folder (`check_sweep_dir`). Another existing file at a single stack's output is replaced; `batch` and `align` never replace a file already there.
- **Globs on Windows** are expanded by the `wild` crate, since `cmd.exe` and PowerShell pass `*` through.
- **Parameter sweeps** (`--sweep-radius`, `--sweep-smoothing` on `depth`) align the bracket once and stack every combination from that alignment, each written into the `-o` folder as `r<radius>_s<smoothing>` (with `_b<blend range>` when `--blend-range` isn't the default).
- **No Library.** A run writes where `-o` says and never opens a Library, which only the app holding its lock writes (ADR-140); `sequence grow` takes frames, not a render. `redraw` reads one render's folder without the lock, so it runs while the app is open, and refuses an output inside that folder.

## Alternatives considered

### `--force` to write over an input

Writing a stack over a photo of its bracket is always a mistake (`deptheos pyramid *.jpg -o out.jpg` run twice in one folder), and source photos are never changed (ADR-142).

### Warn about an irrelevant flag and go on

Failing with status 2 before stacking catches a typo or a misunderstanding at once, not after a GPU run of several minutes.

## Consequences

- A pipeline can hand `deptheos …`'s output straight to `xargs`, since stdout holds only written paths.
- A new option that applies only with another is checked in `Checks` (`args.rs`).

## Code

- `crates/deptheos_cli/src/args.rs`, `main.rs`, `outputs.rs`, `ui.rs`, `interrupt.rs`.
- `crates/deptheos_cli/src/drive.rs` (the one loop driving `batch::Machine` for `batch` and `sequence turntable`), `batch.rs`, `stacking.rs`, `align.rs`, `bench.rs`, `sequence/` (`grow.rs`, `turntable.rs`, `store.rs`).
