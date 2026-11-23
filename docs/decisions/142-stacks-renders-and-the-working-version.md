# ADR-142: Work is stacks of renders, each stack with one working version; a past render is shown, never opened; the workflow rules both apps keep

## Status

Accepted.

## Context

A user stacks a bracket, tries again with other settings or frames left out, compares the attempts, goes back to any of them without losing edits not stacked yet, and batch-stacks a card holding many brackets. Two invariants govern the model: looking at a past render never overwrites what the user is editing, and starting a new stack is always an explicit act. Every feature of either app is checked against the rules below.

## Decision

### The model

- **Frame list** (`deptheos_app::frames::list::List`): the frames of the stack, each included or excluded, with a RAW or JPEG choice where a shot has both (ADR-146). Frames can come from any folder. Excluding keeps a frame in the list without stacking it; removing takes it out.
- **Render**: one finished stack in the Library (`Renders/<id>/`, ADR-140) with the exact frames, inclusions, settings and geometry that made it. A render is immutable. Retouching is a layer saved with it (ADR-148) and never changes the frames and settings it records.
- **Stack** (`deptheos_app::library::stacks`, a `StackId`): one line of work. Its renders are numbered #1, #2, …; numbers never shift and are never reused. The stack's file keeps the last number given (`last_number`, set by `History::claim_number`), so a save needn't read every render. A stack takes the user's name, or is called by the folder of its first frame. Stacks are never inferred from paths or folders.
- **Working version**: per stack, the editable list and settings, and its **base** render. It is **Edited** when it differs from its base by `stacks::Compared` (frames, RAW or JPEG choice, method and slider values, crop, aligner, decoder, Align Frames, frame order). Engine outcomes (frames left out while stacking) and the app's Settings, the preferences among them (ADR-150), never mark a stack Edited. **Stack** makes a new render from the working version and makes it the base. The working version on screen is kept in `list.json`, the others in `Stacks/<id>.json`.
- **New Stack** is the only Stacking command that starts a stack; batches, camera bursts and photos added from the Library make their own. Adding or removing frames, even all of them, stays in the stack. Adding files while the stack has frames offers **New Stack Instead** (ADR-144).
- **The Library is home** on both apps: a tile per stack, grouped by day and batch (ADR-144). Opening a tile opens its stack's working version in Stacking. On the desktop a click selects a tile and a double-click or Return opens it. **New Batch…** starts from the Library, since a batch works on chosen folders, not the stack on screen.
- **Batch** (`deptheos_app::runs::batch`): many stacks in one run, one per folder or split by capture time (`autosort.rs`, ADR-146), without a review step first: a stack split wrongly is fixed afterwards by opening it, editing its list and stacking again. A stack that fails is kept in `Failed/<id>.json` with its frames, settings and error, shown under its batch, to open, fix and try again.

### A past render is shown, not opened

- **Showing a render changes only the picture.** Frames and Stack Settings always hold the working version and stay editable. A strip tile, a Library render row or Swap while comparing shows that render; nothing else changes, so nothing needs a way back. Stack is always "Stack" and always stacks the working version.
- **The render shown is the one stored state; how the working version stands against it is derived.** The desktop stores it as `Renders::active`, changed when an open lands; iOS as `AppModel.result`, changed only by `setResult` once its picture lands. `stacks::Showing` is worked out from the working version, the render shown and the base each time it is needed, never stored:

  ```text
  Nothing ──Stack finishes──▶ Base (the new render)
  Base ──a render picked──▶ Other
  Other(r) ──the base picked, or r deleted──▶ Base
  Other(r) ──Revert to #r──▶ Base (r is now the base)
  Base, edited ──Revert──▶ Base, unedited
  any ──the stack's last render deleted──▶ Nothing
  ```

  The title names the render shown ("#3 · Depth · 44 of 46 frames"), with "Edited" under it while the working version differs from its base. One change line by the working version's controls (under the method on the desktop, just over Stack on iOS) says how the working version differs from the render shown ("1 difference from #3 in the settings", "Same frames and settings as #3"). Slider ticks mark the render shown's values, and dots what changed since the base. Each front end logs the relation at debug when the render shown changes.
- **Revert to #n is the one command that takes a render's setup**, the base's or any other's: render n's frames and settings become the working version, and n its base. It is offered beside the change line, in the Stack menu for the render shown and in a tile's menu for that tile, only when it would change something. It runs at once when nothing unstacked would be lost; otherwise it asks, in the engine's words ("Your 3 changes since #4 haven't been stacked, and will be lost. Every render is kept."), Cancel the default and Revert destructive. A working version whose base the records don't hold (`OtherBase::Unknown`) asks with a body that counts nothing ("Any changes you haven't stacked will be lost. Every render is kept.").
- **Render Info is a render's record, read-only.** It lists `stacks::render_setup`'s rows (method and sliders, alignment, RAW decoder, frames, left out) under the render's title, thumbnail and when it was stacked, marking each row the working version differs in with the edited dot and "Working version: …". It appears only when asked for (a tile's menu, Stack ▸ Render Info, ⌘I), dismisses like a popover and has no buttons: Revert stays with the working version's controls.
- **Compare and Retouch act on the render shown.** The result side of a comparison is the render shown; a tile clicked while comparing shows that render and keeps comparing (clicking the render compared swaps the sides). Retouch opens on the render shown.
- **The model is shared.** The relation, its sentences, the confirmation's words and Render Info's rows are the engine's (`deptheos_app::library::stacks`), so both apps say the same of the same state.

### The rules

The model above gives rules 1 to 8: source images are never written to (only exports the user asks for are written outside the Library, ADR-140); every render is kept and read-only; nothing is lost by opening something (showing a past render, switching stacks or finishing a camera burst keeps every working version); starting a stack is explicit; the Library is home and a tile is a stack; Edited has one definition, `stacks::Compared`; a failed batch stack is kept; a batch runs without a review step. Besides those:

9. **Undo is for Retouch only.** ⌘Z and Undo apply to brush strokes in paint mode and are disabled outside it, so ⌘Z can never undo a list or setting change out of view. Revert has no undo either.
10. **Both apps share vocabulary and behavior.** Terms (Frames, the Library, Stack, Retouch, Edited, Revert, Revert to #n, Render Info, New Batch…), compact setting summaries ("Pyramid · S4") and the day and batch groupings come from `deptheos_app` and `docs/terms-and-usage.md` (ADR-161). Stored preference keys and `@AppStorage` values stay fixed when their labels change (ADR-141).
11. **Only what can't be undone asks.** Navigation can't destroy work, so it never asks. Deleting renders, stacks or failed stacks doesn't ask: it goes to Recently Deleted, with Undo (ADR-140). Emptying Recently Deleted, Delete Library, resetting retouching, and Revert when it would lose unstacked changes ask.

## Alternatives considered

### Infer stacks from exact file sets or from folders

Grouping by file set starts a new stack whenever a frame is excluded; grouping by folder merges unrelated brackets shot in one folder and can't combine frames from several. Explicit `StackId`s from New Stack, Batch and Library imports avoid both.

### Clicking a past render loads its frame list and settings

Combining "look at a render" with "load its settings" would destroy unstacked edits whenever a user clicked an earlier render to compare it. Keeping looking at a render apart from the working version, with Revert to #n the explicit command that takes a render's setup, makes browsing safe without confirmation dialogs.

### A read-only mode for a past render

Clicking a render would swap its frames and settings into locked panels ("Viewing #3") and hold the working version aside until a way back. It is safe, but as a mode of the whole window: every panel has to say so and offer a way out, one render shows two lists depending on how it was reached, Stack needs a second name, taking a setup has two names with different safety, and the working version has no place on screen. A variant that follows the selection while the working version is unedited makes one click do two things depending on state the user may not have noticed. Showing only the picture is as safe with none of that.

### A hierarchy of folders, groups or sessions in the Library

Organizing before the first Stack adds friction. A flat chronological grid of stacks, grouped by day and batch, lets users stack at once and organize by naming and starring.

### Confirmation dialogs or a global undo for list and setting changes

Past renders are read-only and every working version is kept as it changes, so navigation can't lose work; a dialog or a global undo would guard nothing.

## Consequences

- A feature that writes into a source folder, overwrites a working version on navigation, or starts a stack implicitly breaks these rules.
- A past render's frames are seen in Render Info, not in the frame list with thumbnails. The panels can sit beside a picture they didn't make; the change line, the ticks and the title say how they relate.
- Opening a Library render opens its stack showing that render; opening a stack shows its base. Neither changes the working version.
- When a working version's base is deleted, it is based on the newest render left in its stack, and its changes are counted against that render ("Edited since #n"); with none left it has no base.
- Adding or removing frames never changes the render shown: a frame added is shown over it, and the ring, the title and the change line go on speaking of the render.

## Code

- `crates/deptheos_app/src/library/stacks.rs`, `library/stacks/showing.rs`, `library/stacks/setup.rs`, `crates/deptheos_app/src/frames/list.rs`, `crates/deptheos_app/src/runs/batch.rs`, `crates/deptheos_app/src/library/history.rs`, `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`; `docs/terms-and-usage.md`.
- Desktop: `crates/deptheos_gpui/src/app/working.rs`, `src/params.rs`, `src/renders/tiles.rs`, `src/renders/menus.rs`, `src/renders/info.rs`, `src/app/actions.rs`.
- iOS: `crates/deptheos_ffi/src/engine.rs`; `ios/DeptheosMobile/Model/AppModel.swift`, `Model/AppModel+Working.swift`, `Views/RenderInfoView.swift`, `Views/InspectorView.swift`, `Views/SourcesSidebar.swift`.
