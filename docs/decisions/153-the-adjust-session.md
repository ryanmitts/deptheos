# ADR-153: Adjust is one pure state machine shared by both apps; a change is a set then a commit, one preview in flight, undo of whole values for the launch

## Status

Accepted.

## Context

Adjust moves a render's sliders and its reframe (ADR-136, ADR-137) over the stored picture, which it never writes (ADR-152). A drag asks for a new picture many times a second, while a preview takes milliseconds and the full picture of a 45-megapixel render far longer. Its results come back on other threads after the user may have moved on, left the mode, sent the app to the background or retouched the render. Both apps need the same answers to the same questions: when to draw, when to write, what undo takes back, when a render may be deleted. Retouch met these questions first and answered them with a pure machine (`paint::Machine`, ADR-148), as the batch run did (ADR-144).

## Decision

### One machine, events in and actions out

- **`adjust::Machine`** (`crates/deptheos_app/src/adjust/machine.rs`) is one per app. The app's session feeds it each `Event` and carries out the `Action`s back, in order. It does no I/O, keeps no clock and holds no pixels, so its table tests (`machine_tests.rs`) are the behavior.
- **Its lifecycle is one enum** (ADR-143): closed, opening (the reduced copy uploading), ready, away (open with its GPU copies freed, in the background or under memory pressure), failed (Retry makes the reduced copy again) and leaving (its last write or full picture still to come after the user left). While ready, its draws are one more: idle, counting (the full picture on screen as opened, its histogram counted from the reduced copy), scrubbing (a preview in flight), scrubbed (a preview on screen, a drag held still) and settling (the full picture drawing).
- **Every request has an id**: the reduced copy, each preview, each full picture, each write. A result counts only while the state still holds its id; a stale one is dropped and counted in the debug log. The app shows a picture only on the machine's `Show` with its id.

### A change is a set, then a commit

- **`Set` moves the value shown; `Commit` ends the change.** A drag sends sets and commits as it ends; an arrow key, a typed value, the eyedropper, a crop, a paste or a reset sends one set and one commit. So each writes and undoes alike, and the machine never needs to know which control spoke.
- **A commit that moved the value writes once and keeps one undo step**, through the store thread (ADR-152); a commit that moved nothing does neither. No write is per tick.
- **One preview in flight.** A set while a preview draws waits, the newest only, and is drawn as that one lands. A commit asks for the full picture once the preview in flight lands, not before, so a held arrow key keeps showing previews instead of full pictures that a press overtakes. A set while the full picture draws sends a preview and makes that picture stale.

### Undo, writes and leaving

- **Undo and redo step through whole values**, one per committed change, up to `KEPT_STEPS` (500) a render, the oldest going first. Each step writes. An undo during a change commits it first, so the change is what it takes back. Reset and paste are ordinary changes (the rules of ADR-144 make their values).
- **Undo lasts the launch, per render.** The machine keeps each render's steps when Adjust leaves it and gives them back when it opens again, as Retouch keeps its own (ADR-148).
- **Leaving commits a change in progress**, then waits for the render's last write, and for a full picture of the saved value when a preview is on screen and the GPU copies are still there; otherwise its report tells the viewer to draw the render again. Opening another render leaves the first.
- **Writes outlive the session.** The machine knows each render's latest write until it lands. A delete takes `AdjustClosed`, which the machine gives only when none of the renders is open, being left or has a write pending, as a paint delete takes `PaintClosed`. It also says whether a render has a write pending (`is_writing`), which an export of it waits for.
- **Away and back.** Going to the background frees the GPU copies and keeps the values, which may still change and be written; coming back makes the reduced copy again, then draws the full picture if what is on screen is no longer the value. Memory pressure is a shed (`Event::Shed`): a moderate one while ready frees only the preview's copies at the sizes drawn (`Action::TrimPreview`), which the next tick makes again, and a critical one goes away as the background does, until the app says memory is no longer short (`Back`). A change to the stored picture (its generation number moved by Retouch) makes the reduced copy and the full picture again.

### The session that drives it

- **`AdjustSession`** (`crates/deptheos_app/src/adjust/session.rs`) is one per app, beside `PaintSession`. It feeds the machine under one lock and delivers its updates in order outside it, so an observer that calls back in can't deadlock. Its calls are plain (open, set, set a slider, commit, undo, redo, reset, paste, eyedrop, leave, away, back, shed, set hot), so the bridge can mirror them.
- **Its requests run on the engine's threads** (ADR-143): an open's read on the store thread, after the writes queued before it; the GPU copies made, each preview and full picture drawn, and the eyedropper's sample on the pool's interactive lane; each write on the store thread. On iOS the copies are made only if `engine::memory::limit()` holds them; otherwise the open fails with a worded reason.
- **A damaged adjustments file opens neutral** and says so; an unreadable one doesn't open.
- **The eyedropper** (`adjust/eyedropper.rs`) maps the click back through the reframe to the stored pixels with paint, averages a small patch, refuses a clipped or dark one, and sets the white balance that makes it neutral: through the DNG SDK's camera neutral for a camera RAW stack, by inverting the resolve's adaptation for a standard one. It is one set and one commit.
- **Leaving a render it wrote draws its thumbnail again** under its adjustments (ADR-140).
- **A delete's close covers Adjust.** `History::close_paint` asks every session of the Library to leave the renders it adjusts and waits for their last writes, as it waits for the paint worker (`adjust::session::close`, never on the store thread); its `PaintClosed` carries the `AdjustClosed`, so a delete can't be sent without both.
- **The bridge mirrors it** (`crates/deptheos_ffi/src/adjust.rs`): `Engine::adjust_session` makes one per app over an `AdjustObserver`, and every call only queues work, as the paint session's do. The panel crosses worded (`AdjustPanel`, each slider's label, help, range and step), and a value crosses as an `AdjustValue` the app reads slider by slider (`get`, `text`, `parse`, `position`, `value_at`), so Swift has no code per slider. A preview crosses as an `AdjustPreview` whose pixels the app reads in place and whose buffer goes back to the session when the app lets go of it (ADR-139). Copy and Paste onto a render not open, and a thumbnail drawn again after Retouch, are the engine's async calls (`render_adjustments`, `paste_adjustments`, `rewrite_thumbnail`).
- **The paint comes from the Library's paint worker** (`paint::worker::flatten`), so the GPU copies, the eyedropper and the thumbnail are of the picture with its retouching, and the app passes no paint in.

### What the machine doesn't own

Exclusivity with Retouch is each app's mode enum, which holds either session. The desktop's is `Mode` (`crates/deptheos_gpui/src/app/mode.rs`): viewing, Retouch with paint's `Painting`, or Adjust, changed only by `AppView::set_mode`, which closes the one open before opening the next and moves the viewer's place and both sessions with it. The desktop's session lives in the `Adjust` entity (`crates/deptheos_gpui/src/adjust.rs`), one per window beside `Paint`, which outlives each open as the session does; `Mode::Adjust` holds the render open. The entity keeps what the session last said and one track per slider of the engine's `Panel`, each running 0 to 1 through `position` and `value_at` so the Kelvin track is the engine's. A drag sends a set per move and a commit on the track's release; an arrow key, a typed value, a reset and the eyedropper each send a set then a commit, or the session's own one-change call. The reduced copy, the preview's tiers and its budgets are the session's and the renderer's (ADR-135); what the viewer shows is `View`'s (ADR-149).

## Alternatives considered

### A write per drag tick, or a save timer

A tick-by-tick write queues dozens of writes a second behind the store thread for a value that a later one replaces; a timer writes a value the user hasn't finished with and still leaves a window where a quit loses the last change. A write as each change ends is what the user would undo, so the two are one.

### Undo of slider deltas

A list of "Exposure +0.3" steps has to be replayed in order and breaks when a value is clamped or a process version changes. Whole values are small (a few hundred bytes), so undo just puts one back.

### The full picture on every commit, at once

A held arrow key commits many times a second; drawing the full picture at each would overtake it again and again and show nothing new until the key is let go. Waiting for the preview in flight keeps the screen moving.

## Consequences

- Both apps drive the same machine, so a desktop drag and an iPhone drag write, undo and leave the same way.
- An app must send a commit as every change ends; a control that forgets leaves a change unwritten until the next commit, undo or leave.
- Undo is lost on quit, as Retouch's is.

## Code

- `crates/deptheos_app/src/adjust/machine.rs`, `crates/deptheos_app/src/adjust/machine_tests.rs`.
- `crates/deptheos_app/src/adjust/session.rs`, `crates/deptheos_app/src/adjust/session_tests.rs`, `crates/deptheos_app/src/adjust/eyedropper.rs`.
- `crates/deptheos_app/src/library/adjustments.rs` (the writes it asks for).
- Desktop: `crates/deptheos_gpui/src/adjust.rs`, `crates/deptheos_gpui/src/app/adjusting.rs`, `crates/deptheos_gpui/src/app/mode.rs`, `crates/deptheos_gpui/src/app/tests/adjust.rs`.
