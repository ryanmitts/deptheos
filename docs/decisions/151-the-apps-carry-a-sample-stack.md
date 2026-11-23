# ADR-151: Both apps carry two sample stacks, each copied into a new stack that waits for Stack

## Status

Accepted.

## Context

Someone opening either app for the first time has nothing to stack, and the website's samples are 250 MB and up. An empty Library has to say what the app is for. An App Review tester is the same newcomer with ten minutes, and an app whose main function can't be exercised is rejected.

## Decision

- **Two samples, in the repository once, each at half size (4096×2732) as JPEG, made by `script/sample-stack.swift`**, which keeps what Render Info shows and drops serial numbers and the owner's name.
  - **`samples/hornet`, Hornet:** the gallery's hornet, all 66 frames, handheld, quality 70, 64 MB. It shows the aligner at work.
  - **`samples/pcb`, Circuit Board:** a circuit board on a tripod with a focus rail, all 200 frames, quality 35, 68 MB. It shows a long, deep bracket, and the speed. At quality 35 the frames show no blocking and the stacked result is, at 100%, the one from quality 70 (174 MB) with a little less grain; a quarter size at the same weight (55 MB) is visibly softer.
- **Every bundle carries both.** The iOS app as folders in its bundle (folder references to `../samples/hornet` and `../samples/pcb`); the desktop in `Contents/Resources/samples` on macOS, `share/deptheos/samples` on Linux and `samples` beside the exe on Windows. `script/sample-check.sh` fails a bundle made without every frame. A desktop build with none (`cargo install`) keeps the link to the website.
- **Try a Sample Stack copies the frames, then opens them as a new stack.** A stack keeps its frames' paths. An iOS app's bundle path changes with every update, and a Mac app is translocated, moved or replaced, so frames read where they ship would go missing. The copy is in iOS's Imports (`Imports/Hornet`, `Imports/Circuit Board`, so Settings' unused copies treat it as any copy) and in the desktop's Library folder (`Samples/Hornet`, `Samples/Circuit Board`). The catalog and the copy are the engine's, the same on both apps (`deptheos_app::frames::sample`; iOS through the bridge's `Engine::copy_sample`), each app naming its place: the free space is checked first, the copy is made whole or not at all (into a hidden folder, renamed into place once every frame is in), and a whole copy already there (its frames at their sizes and nothing else) is used again, else a new one goes beside it (`Hornet 2`). Each try is still a new stack, never frames added to the one on screen (ADR-142).
- **Nothing is stacked until Stack.** The newcomer lands on one frame as the camera took it, mostly out of focus, presses Stack and watches it become sharp: the before is what makes the result mean something, it is the same step as with frames of their own, and the method is still theirs to choose.
- **The stack is named for its folder, "Hornet" or "Circuit Board"**, as any stack is until renamed.
- **Where they are offered.** Try a Sample Stack offers the two by name, each with its frames and how it was shot. iOS: the empty Library and the Library's + menu. Desktop: the welcome, and Help ▸ Try a Sample Stack.
- **iOS's empty Library says what the app is for**, in the desktop welcome's words: "Add a Focus Bracket", "The photos of one subject, each focused a little nearer or farther than the one before. Deptheos stacks them into one picture, sharp throughout.", with Add Folder… (leading, since a folder is read where it is and nothing is copied), Add from Photos…, Add from Files…, Take Burst and Try a Sample Stack.
- **The lifecycles** are `SampleCopy` (iOS) and `SampleOpen` (desktop), each in CLAUDE.md's table.

## Alternatives considered

### A dozen frames at a smaller size

Every sixth frame at 2400 px is 5 MB and stacks to a convincing picture. It is not a bracket anyone would shoot, the rail has eleven ticks, and at 100% the result has a phone screenshot's detail. The apps exist to stack real brackets fast; the samples should be real ones. The cost is 132 MB in each download and in the repository.

### Fewer of the board's frames

Every third of its frames (67, 58 MB at quality 70) stacks to the same picture at this size. All 200 at a lower quality weigh about the same and keep the bracket as it was shot, with its rail of 200.

### The spider

All 46 frames at half size are 36 MB. The hornet's result is crisper at 100% and keeps the whole frame, where the spider's loses about 15% to alignment; the hornet is also the hand-held one, which is how a phone's burst is shot.

### Downloading it on first use

Keeps the bundles small, and makes the first run need the network, a host that stays up and a progress bar, in apps that otherwise make no request (iOS) or one (the desktop's update check). The privacy page gets to stay one sentence.

### Reading the frames from the bundle

No copy and no space. The stack's frames go missing at the next update (iOS) or the next launch of a translocated app (macOS), and inside an app is no place to Locate them.

## Consequences

- Each download and the repository carry 132 MB of sample frames.
- A sample's copy takes its space once, however many stacks are made of it.
- A bundle made without every frame fails its build (`script/sample-check.sh`); a desktop build with none links to the website instead.

## Code

- `crates/deptheos_app/src/frames/sample.rs`, `crates/deptheos_ffi/src/sample.rs`.
- `crates/deptheos_gpui/src/app/sample.rs`, `ios/DeptheosMobile/Model/AppModel+Sample.swift`.
- `samples/README.md`, `script/sample-stack.swift`, `script/sample-check.sh`.
