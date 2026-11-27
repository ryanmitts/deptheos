# Terms and usage

This document defines the vocabulary, capitalization, punctuation, error structure, and plural rules for every user-facing string in Deptheos (see ADR-142, ADR-161, ADR-149 and ADR-163).

## 1. How to use this

This covers every string a person can read in the desktop app, the iPhone and iPad app, the `deptheos` command line and `stackrig`: labels, menus, tooltips, VoiceOver labels, notices, errors and help text. The website uses the same terms. Before you add or change a string, look the word up in section 2 and the form in sections 3 to 5. Spelling is American throughout, in the apps and on the website.

## 2. Terms

"Don't use" lists competing words so you can grep for them. A word listed there may still be right in another sense; the Notes say which.

### What the user works with

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| focus bracket | The photos shot of one subject at stepped focus | focus stack (for the photos), bracket alone in the apps | "Bracket" alone is fine in prose after the first use and as the iOS camera mode. "Focus stacking" names the technique. |
| frame | One photo of a bracket, in a list or a stack | image, photo, picture, shot, file (for one input) | "photo" for the user's originals and the iOS Photos library; "file" when the format or the disk matters. |
| shot | One capture moment (a RAW + JPEG pair is one shot) | frame (for the pair) | For capture time and auto sort. |
| stack | One subject's frames and every render made from them; one tile in the Library | project, session, set, focus stack | Also the verb and the **Stack** command. Don't use it for the result picture ("the stack is judged at 100%" → "the result"). |
| render | One finished stack, numbered in its stack | version, output, stacked image | Adjective: "a Pyramid render". |
| #n | A render's name: its number in its stack | Render n (on screen), render {id}, a method letter | "#4", "#4 · sharper" when named. A stack's only render is "#1", numbered like any other. Methods have no letters or glyphs, so a render is never named by its method. A star goes right after the number: "#4 ★ · sharper · Pyramid · S4". VoiceOver reads "Render 4" (section 3). |
| render glyph | The Stack glyph with its top layer filled: marks a render | a method's glyph, a picture icon | Beside frames, so a render reads apart from them; alone in a chip or pill; and where renders are counted. Never in a sentence that already names the render ("#4"). |
| result | The picture a render holds | output, stacked image, the stack | "the stacked result" when retouching makes the distinction. |
| original | The result as stacked, before retouching | (iOS "the originals" for source photos) | For source photos, say "the photos", not "the originals". As a source in Retouch it is "Original" ("Source · Original"), and the home control's menu offers it as "Original (paint it back)". Not "Eraser" for the source: Eraser is the tool that paints it. |
| working version | The frames and settings being edited; the Frames list and the inspector (iOS: Frames and Stack Settings) always hold it, and the next Stack uses it | current setup, workspace, draft | No command, tile or mode is named after it: Stack stacks it, Revert to #n replaces it, and showing a render leaves it as it is (ADR-142). Label: "Working version" (the Library's box about a stack's kept one; in Render Info, "Working version: …" under a row it differs in). |
| batch | Many stacks made in one run with one method | queue, job list | "Batch" as the feature name. |
| Starred / star | A render marked to keep | favorite, keeper, flagged | Commands "Star Render", "Unstar Render". In a render's title the ★ goes right after its number (see #n). |
| adjustments | A render's look: its adjustment sliders and its Crop, set in Adjust and drawn over the result, never written into it | edits, develop settings, look (on screen), preset | They belong to a render, never to the working version, so Edited keeps its meaning. A new render takes its base render's. Commands "Copy Adjustments" and "Paste Adjustments"; Paste leaves the Crop. |

### Screens and panels

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| the Library | The home screen: one tile per stack | history, gallery, Renders (for the screen) | Always capitalized, with "the". |
| the Stacking screen | The screen showing one stack | stacking screen, the viewer (for the screen) | "Stacking" alone in a tooltip: "Open the selection in Stacking". |
| Frames list / Frames | The left panel listing a stack's frames | sidebar, frame list | "the list" in prose once the reader knows it. On iOS it is the Frames side of the sources (Frames \| Renders), in the column at regular width and the Frames sheet on iPhone. |
| Frames \| Renders | The switch at the head of a stack's sources: its frames or its renders | Sources, History, tabs | Two segments, the words alone. iOS: at the head of the Frames column and sheet, and of Retouch's column ("Paint From" over it at regular width). Desktop: under Paint from in Retouch, with "Click a frame" or "Click a render" beside it; outside Retouch the desktop's renders are the Renders strip, which Retouch puts away. |
| inspector | The right panel: method, alignment, RAW, brush on the Stacking screen; the selected stack and its renders on the Library | sidebar, Settings (desktop), parameters, details pane, Details | Lowercase in prose; "Inspector" in the View menu. One panel on every desktop screen, shown or hidden together (⌃⌘I). iOS calls its panel "Stack Settings" (the inspector at regular width, its own sheet on iPhone, titled "Stack Settings" with Stack in its bar and the frame count under the title); in Retouch it holds the brush and is called "Brush". |
| Renders strip | The desktop's strip of renders under the viewer | history, timeline, This Stack | "the strip" after first use. Not in Focus Mode or Retouch, where the renders are the Renders side of Paint from. iOS has no strip: its renders are the Renders side of the sources (Frames \| Renders), one row each, and the menu of the render shown in the view bar lists them too. |
| sheet (iOS) | What opens over the photo on iPhone from a bar button: Frames, Stack Settings, or Brush in Retouch | panel, drawer, popover | Each button opens its own sheet and closes it again; another button changes what the sheet holds. It covers the photo and the controls under it; the photo never resizes for it. Close is at its leading end. The Brush sheet opens with Brush \| Eraser. Nothing opens over a sheet: Render Info and Add Frames go into it, with the way back. |
| view bar | The controls along the bottom of the photo: the render shown, the focus rail and the lenses (Compare, Contribution, Depth Map) | toolbar, HUD, deck | The same parts on both apps. The desktop adds zoom and Focus Mode, with the lenses as buttons; iOS's Stacking holds them in one view menu, and ends with Stack, and on an iPhone upright it is two rows (ADR-149). |
| view menu | iOS's one menu under the photo: in Stacking, Result and the four layouts, Compare With, Contribution and Depth Map; in Retouch, Painted and its three layouts and Depth Map | lens menu, More, Options | Lowercase in prose; "View" for VoiceOver. Its button shows the symbol of what is on, lit. Not the menu bar's View menu, which has the same commands with their keys. |
| home control | The view bar's first control: the render glyph and the render shown ("#4 ★"; "Result" before it is saved), a click to show the result alone, and a menu of the stack's renders | Home, Back, Show Result | The docs' name only: the screen shows the render. Tooltip "Show the result alone"; in Retouch, on both apps, "Paint from the original, to paint it back" and its menu's first item "Original (paint it back)". It replaces iOS's Show Result. |
| focus rail | The view bar's row of ticks, one per frame of the render shown, to move through the frames by dragging | scrubber, slider, sparkline, histogram, timeline | The docs' name; on screen it has no title (VoiceOver: "Frames", valued "Frame 18 of 46, IMG_0418.DNG"). Its label is "Frame 18 · IMG_0418.DNG · 14%", "Frame 18 · IMG_0418.DNG · left out", or with no share when the render keeps no frame map; its readout "18/46" ("–/46" for none). Nothing on or near it says a frame is weak or could be left out (ADR-149, ADR-121). |
| share | How much of the result a frame supplied, as a percentage: a tick's height and the rail's label | contribution (for the number), detail, weight, score | "14%", "<1%" under one percent, "0%". A fact about the result, never advice about the frame. |
| viewer | Where the picture is shown | canvas, photo, preview | "the picture" for what is in it. |
| Settings | The app's preferences window | Preferences, Options | macOS: "Settings…" (⌘,) in the app menu. iOS: a sheet opened by the Library's gear, titled "Settings", with Export, Alignment, Retouch (iPad), Storage, Diagnostics and About Deptheos; the preferences both apps have use the same words (ADR-150). On iOS the stack's panel is "Stack Settings", never "Settings", and the system's app is "the Settings app". |
| Export sheet | Where an export's format, name and folder are set | Export dialog, Export window | |
| Batch dialog | Where a batch is set up (desktop) | Batch window | iOS: "the Batch sheet", title "New Batch". |
| paint mode | The state in which a render is retouched | retouch mode, edit mode | Lowercase. Title bar: "Retouching #4". |
| Adjust | The mode in which a render's adjustments are set; it opens on a saved render, and not while retouching | Edit, Develop, Edit mode | "Adjust" for the command, "adjust" for the act. Not "edit": Edited is the working version's. |
| GPU | What stacking runs on | graphics card, video card, adapter, device (in the apps) | The website may say "graphics card" where it first explains it. |

### Methods and their settings

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| method | Pyramid, Depth or Wavelet | algorithm, stacker | CLI flag `--stacker` stays; its help says "method". |
| Pyramid | The default method | Laplacian pyramid, pyramid (lowercase) | Capitalized everywhere, including mid-sentence ("a Pyramid render"). |
| Depth | The method that picks the sharpest frame at each spot | Depth map (as the method), depth method | |
| Wavelet | The method that keeps each pixel's color whole | complex wavelets, wavelet (lowercase) | |
| Smoothness | Pyramid's slider | smoothing (for Pyramid), blend | Ends "Crisp, more noise" / "Calm, softer". |
| Smoothing | Depth's main slider | Smoothness (for Depth) | Summary "Depth · R S B": S is Smoothing. Ends "Narrow outlines" / "Even backgrounds". |
| Radius | Depth's slider for the area that decides each spot | patch size, window | Under Smoothing, always shown. Ends "Thin detail" / "Steadier". |
| Blend Range | Depth's slider for how much neighboring frames mix | blend, snapping | Depth's one setting under Advanced. Ends "One frame per spot" / "Mixed, softer". |
| Reset to Defaults | Put the method's sliders, Advanced's too, back to the app's defaults | Revert (for that), Restore Defaults | Desktop button under the sliders; double-clicking a slider's name resets that one ("Double-click to reset to 8"). iOS: "Reset Settings". Never a render's values: that is Revert. |
| Denoise | Wavelet's slider | noise, consistency | Ends "All detail" / "Less noise". |
| adjustment sliders | Adjust's sliders: Exposure, Temperature, Tint, Contrast, Highlights, Shadows, Whites, Blacks, Clarity, Texture, Vibrance, Saturation, Vignette, with its Midpoint, and Sharpening, with its Radius, Detail and Masking | sliders (alone, for these), controls, settings | "the sliders" alone means the method's. Names keep their capitals, as a method slider's do. Temperature reads in kelvins for RAW stacks and from -100 to 100 for others. |
| Crop | Adjust's tool that cuts the result to a rectangle, with quarter-turns and flips | Trim, Reframe (on screen), crop (for the switch above) | Stacking's crop is always written whole, "Crop to the area every frame covers", so "Crop" alone is Adjust's. Code and records call it the reframe. |
| depth map | The map of which frame each spot came from; Depth Map in the view bar (iOS: in the view menu), View ▸ Show Depth Map, or M | sharpness map | Lowercase in prose, "Depth Map" as the view name. |

### Alignment

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| align / line up | Bring each frame into place | register, warp, match | "line up" in plain sentences, "align" in labels and progress ("Aligning 12 of 46"). |
| aligner | ECC or Grid search | alignment method, grid aligner | Prose: "the ECC aligner", "Grid search". |
| ECC (affine) / Grid search (similarity) | The two aligners' labels | Chained ECC, coarse-to-fine, grid, grid alignment | The bracketed words stay only in the picker. Compact summaries say "ECC" or "Grid search". |
| Crop to the area every frame covers | The crop switch | Crop to Aligned Area, Crop to the aligned area, valid area | Summary word: "uncropped". Not Adjust's Crop. |

### Files, formats and color

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| RAW | Camera raw files, as a kind | raw, Raw, camera RAW/DNG | "RAW or DNG frames"; never a slash pair. |
| DNG / linear DNG | The RAW-stack export | DNG output | |
| JPEG, PNG, TIFF, HEIC | Standard formats | JPG (except `.jpg`), HEIF in the apps | "standard images" only beside RAW ("a stack must be all RAW and DNG or all JPEG, PNG and TIFF"). |
| color profile / camera profile | Embedded profile / Adobe camera profile | ICC profile (in the apps), matrices | American spelling. |
| capture time | When a shot was taken | when the photos were taken, timestamp | |
| folder | A place on disk | directory | CLI help too. |
| Show in Finder / Show in Explorer / Show in File Manager | Open the folder holding a file | Reveal in Finder, Open Containing Folder, Show Folder | Finder on macOS, Explorer on Windows, File Manager on Linux; prose "the file manager". |
| Library folder | Where renders and stacks are kept | History folder, history, store | |

### Actions (verbs)

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| Add | Bring frames or folders into a list | Import, Open (for frames) | "Add Folder…", "Add from Photos…". On iOS "Add from Files…" picks single frames, which are copied into Deptheos, and "Add Folder…" one folder, used where it is (ADR-145). |
| where they are | Frames read from the folder they were picked in, not copied (iOS) | in place (on screen), linked, referenced, external | "used where they are", "Deptheos reads the frames from T7 SSD each time", the Add Frames sheet's "Use Them Where They Are…". Settings ▸ Storage ▸ Folders lists these folders; the docs and code may say "in place". |
| Stack | Make a render from the working version | Run, Process, Render, Stack Working Version | Always the one word: the frames and settings on screen are always the working version's, whatever render is shown. |
| Export | Write a render to a file or place the user chose | Save (for that write), Write | iOS "Share…" and "Save to Files…" are system words and stay. |
| save / keep | What the app does by itself with renders | export (for that) | "Saving the render…"; "Every render is kept". |
| Delete | Remove renders, stacks or copies from disk | Remove, Discard, Erase | Alert bodies say "deleted", not "removed from disk". |
| Remove | Take frames out of a list, folders out of a setting | Delete (for frames) | "Remove from List", "Remove All Frames". |
| exclude / include | The user's per-frame switch | skip, disable, hide | |
| Revert | Make a render's frames and settings the working version, and that render its base: the one command that takes a render's setup, for the base and any other render alike | Reset (for that), Undo, Edit from, Start From, Use, Restore, Load | "Revert" beside the change line; "Revert to #4" in the Stack menu (for the render shown) and a render's menu. Offered only when it would change something. It runs at once unless unstacked changes would be lost; then it asks, in the engine's words ("Revert to #4?", Cancel and Revert). No ellipsis, though it may ask: what it asks is only a confirmation, and only sometimes, never more input (section 3). |
| Reset… | Take retouching back to the stacked result | Clear, Revert | iOS "Reset Retouching…"; "Reset Settings" on iOS resets the stack's sliders. |
| Open | Show a stack in Stacking, or a render on screen | View as Stacked, Open (and Use Its Settings) | One word for every open. Opening a render changes only the picture: the frames and settings stay the working version's. A Library render opens its stack showing that render; a stack opens showing its base. |
| Render Info | A render's record, read-only: its method and sliders, alignment, RAW decoder, frames and what was left out | Details, Setup, Properties, Get Info | The same on both apps: a render's menu, Stack ▸ Render Info and ⌘I, and on iOS the title menu too. A row the working version differs in is marked with the edited dot and "Working version: …". It has no buttons: Revert stays with the working version's controls. |
| Compare | Lay the result out with a frame or another render, in the layout used last (Split to start); off, the result alone | Compare Mode, Reference, Before/After | One word, Title Case, the same on both apps. The desktop's view bar button has a menu of one list: Off, Split, Side by Side, Top and Bottom, Flicker, the one on screen checked. On iOS the list heads the view menu, and the menu bar's View menu has Compare itself. "Off" is the result alone. In Flicker iOS adds Flip (switch to the other picture) beside the view menu, and Flicker picked again flips; the desktop flips with each press of Flicker and keeps Swap while a render is compared. |
| Compare With | iOS's view menu: what the result is compared with, the frame on the rail or one of the stack's other renders | Reference, Compare Against | Title Case, a submenu shown only where the stack has another render; it names what is compared with now under its title. The menu of renders picks the render looked at, never what is compared with. |
| Contribution | Show what the result took from the frame on screen as it is, the rest dimmed (C) | Sharpness overlay, Highlight, tint | A button in the desktop's view bar, an item of iOS Stacking's view menu; on only while a frame is on screen, alone or compared with; it stays on from frame to frame, and Depth Map turned on from it opens lit for that frame. A render, a frame left out, a stack without a frame map and Retouch have none (ADR-149). |
| View (the menu bar's menu) | The menu bar's menu of every view, with its key: Result, the layouts, Depth Map, Contribution, zoom | More, Options | The view bar has no View menu of its own (ADR-149): on the desktop its lenses are buttons, iOS's view menu has the lenses but no zoom or keys, the desktop's Zoom to Fit and Actual Size head its zoom presets, and on iOS a double-tap goes from Zoom to Fit to Actual Size (100%) at the point tapped and back, and a pinch goes to 800%, or out to a quarter of fit; every iOS view shows the picture's own pixels at any zoom (ADR-164). iOS's View menu, for a keyboard, adds Previous Frame and Next Frame (← →) and Hide Controls / Show Controls, while a tap on the photo hides and shows them. |
| Zoom to Fit | Fit the picture to the view (F) | Fit, Fit to Window | The same words on both apps. |
| Focus Mode | The desktop's view: only the picture and the bars (Tab) | Full Screen, Zen | The desktop's only. iOS hides the controls over the photo with a tap (with a keyboard, View ▸ Hide Controls), and its columns with the system's buttons. |
| Compare with Result | Show a render beside the result on screen, the render shown | Set as Reference, Reference A, Compare with Working Version | The same words on both apps; the desktop adds its shortcut, "(⌘-click)". While comparing, a click on a render shows it and keeps comparing; on the compared render, the sides swap. |
| Retouch / paint | Open paint mode / apply pixels from a source | edit, brush in, clone | "Retouch" for the command, "paint" for the act. What it paints from is picked under "Paint from" (iOS "Paint From", a column title) with the Frames \| Renders switch, or on the focus rail. |
| Painted / Source · Frame 23 | The labels of Retouch's two pictures: the painted result, and what the brush paints from ("Source · #3", "Source · Original") | Result (for the painted one), Reference | "Painted" is also the layout of the canvas alone, beside Split, Side by Side and (iOS) Top and Bottom. |
| Sharpest Frame | iOS Retouch's button that makes the next tap on the photo paint from the frame the result took that point from | Eyedropper, Pick, Auto Source | Title Case as a button name; its hint "Tap the photo to paint from the frame sharpest there". The desktop has no button: ⌥-click does it ("Paint from the sharpest frame there"). |
| Locate… | Point the list and renders at a moved folder | Find, Relink, Reconnect | "relinked" only in the result: "The renders were moved to the new folder". |
| Cancel | The control that stops a stack, batch or sequence | Abort, Stop (as a control) | Tooltip: "Cancel the stack". "stops" as the verb in prose. Its key is ⌘. (Ctrl+. on Windows and Linux), never Esc, which leaves Compare and clears the Library's selection while a run goes too. |
| Try Again / Open to Retry | Run a failed stack again / open it to fix and retry | Retry, Tap to retry | |

### Outcomes and states

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| finished / done | A stack, batch or export completed | succeeded, successfully | "Stacked in 12 s"; "Batch: 12 of 14 made · 2 failed". |
| Failed / didn't finish | A stack that ended with an error | errored, crashed, aborted | |
| canceled | Stopped by Cancel; what was made is kept | aborted, interrupted (for Cancel) | American spelling. |
| left out | A frame the app couldn't read or align, so it stacked without it | dropped, skipped (for frames), rejected | "Left out 2 frames: …". |
| skipped | Files or folders auto sort didn't use | left out (for auto sort), ignored | |
| Missing | A frame whose file isn't where the list says | can't be found (as a label), gone | Message: "1 frame can't be found". iOS "can't be reached" and "Not connected" when the drive is unplugged. |
| On T7 SSD / T7 SSD Not Connected | iOS's badge at the head of the Frames list saying where the frames are read from | Stored on, Location, Offline | "On" with the drive's or server's name, "On the server" or "On iCloud Drive"; its menu offers Copy Frames into Deptheos. While the place is away, "T7 SSD Not Connected" ("Server Not Connected", "iCloud Drive Not Connected"), a title, with Locate… in its menu. None for frames in Deptheos or elsewhere on the device. The docs call it the storage badge. |
| paused | A batch the app held while it couldn't use the GPU (iOS in the background); the iOS camera held by the system | suspended, waiting | The user never pauses: there is no Pause control. |
| Edited | The working version has changed since its base render | Modified, Changed (on screen), Unsaved | Under the title after the render shown's number ("#4 · Edited" on iOS; a third title line with the edited dot on the desktop), as a pill on the base's tile, and on the stack's Library tile: there it is "Edited" alone. The desktop's Library details write it out whole in the working version's box: "Edited since #4. Open the stack to go on." for the stack open in Stacking, and the count for one kept off screen ("2 changes since #4. Open the stack to go on."). VoiceOver gives it as the base tile's value (section 3). Nothing is unsaved: the working version is kept by itself. |
| read-only | Render Info, a render's record | locked, frozen | There is no read-only mode: showing a render never locks the Frames list or the settings, so no screen says "Read-only" or "Viewing #3" (ADR-142). The desktop's "Locked while stacking" is the one lock. |

### Batch and sequences (desktop and command line)

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| auto sort | Split each folder into stacks by capture time | auto split, smart sort | "Auto sort" as the option, sentence case. |
| One stack per folder | The other batch split | One per Folder | |
| Batch n of N | Batch progress | job n, item n | "Batch 2 of 14 · bee". Delete items count stacks: "Delete Batch (3 Stacks)…", not "Items". |
| grow sequence | Numbered frames where frame k stacks k frames | grow movie, build-up | "Export Grow Sequence…". |
| turntable sequence | One stack per angle, turned into frames | rotation, spin | Modes "Full", "Grow across stacks", "Both". |

### Camera (iOS)

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| burst | What the camera takes for one stack | capture, shoot | "Take Burst"; "The burst is saved in Captures". |
| Sweep / Bracket | The two camera modes | Focus sweep, Auto | Capitalized as names. |
| Captures / Imports | The app's folders for bursts and copied photos | Camera Roll, Inbox | Folder names, capitalized. |
| Photos / Files / Share… | The system apps and sheet | photo library (except the permission prompt), Files app | System words: keep Apple's. |

### Command line

| Use | Meaning | Don't use | Notes |
|---|---|---|---|
| `deptheos` | The program | the CLI (in help), the binary | "the command line tool" in the app menu. |
| subcommand / method command | `pyramid`, `depth`, `wavelet`, `batch`, `sequence`, `bench` | verb, mode | |
| option | A flag such as `--align` | flag, switch, argument (for `--x`) | |
| sweep | Several values of one setting, one run each | grid, range run | Not the camera's Sweep. |
| `error:` / `warning:` | The prefixes of stderr lines | `Error:`, `ERROR`, `Note:` | Lowercase, then the message in the pattern of section 4. |

## 3. Writing rules

### Capitalization by kind of string

- **Title Case**: menu items, buttons, command names reused as tooltips ("Show or Hide Frames"), view names ("Side by Side"). Lowercase articles, coordinating conjunctions and prepositions of four letters or fewer ("Remove from List", "Sort by Capture Time") unless the word is last or part of a phrasal verb ("Zoom In", "Stack With").
- **Sentence case**: setting labels, switches, pickers, filters, status, notices, errors, sentence tooltips, alert titles ("Delete bee #4?"), empty states, headings, and desktop window and panel titles ("How to split"). iOS navigation titles, section headers and empty-state titles are Title Case.
- **A window or file dialog named after the command that opens it** takes the command's name as the menu writes it, Title Case, without its ellipsis: "New Batch", "Export Grow Sequence", "Add Folders", "Add Subfolders of", "Rename Stack", "Save Diagnostics". Any other window or dialog title is sentence case: "Choose where the renders go", "Export to".
- **Always capitalized**: Deptheos, the Library, Stacking (the screen), Batch (the feature), Focus Mode, method names, RAW, DNG. A slider's name is a name and keeps its capitals wherever it is written ("Smoothness", "Blend Range"); a switch's label is a sentence ("Crop to the area every frame covers").
- **Short status notes** beside a control ("Needs one stack per folder") are sentence case with no full stop.
- **A setting's help line** is one sentence saying what moving the control does ("Higher removes more noise, and more of the faintest texture with it."), in words a photographer uses, never the algorithm's. It is written once, in the shared file, and shown as its section footer on iOS. On the desktop a slider's and the crop switch's are tooltips (the words at a slider's ends say the trade in the panel), and the aligner's and the decoder's show under their controls in the inspector's Alignment row, once it is open.
- An error shown on its own starts with a capital.

### Punctuation

- **Full stops** on alert bodies, empty-state bodies, iOS section footers, help text, errors, and every sentence of a string with two or more. None on labels, buttons, menus, titles, status lines and one-clause tooltips or banner notices.
- **Ellipsis**: one `…` character, never on a control that only opens a menu (Compare's menu, "Choose"); on a command that asks for more before acting ("Export…", "Delete Failed Stack…" on both apps), and on progress that shows no count ("Saving…"; "Aligning 12 of 46" has none). "Check for Updates…" and "Report a Problem…" keep theirs as the names macOS users know. A command that asks only sometimes, and then only to confirm, takes none: "Revert to #3" runs at once unless unstacked changes would be lost, so most of the time it asks nothing. Not as a pause mid-sentence. A sentence that ends on a command name ending in `…` takes no extra full stop ("find them with Locate…"); better, reword so the name isn't last.
- **Apostrophes and quotes**: straight `'` only. Names of files and folders in straight double quotes: `Locate "bee"`.
- **Middle dot** ` · ` with spaces between items in compact metadata: "#4 · Pyramid · 46 frames", "Batch 2 of 14 · bee". iOS too, not a comma ("Pyramid · uncropped").
- **Colon** joins a lead to its detail ("Left out 2 frames: …"); one per line. A second colon means the string needs two sentences.
- No dashes as punctuation; the en dash only in compact ranges ("1–100"). No exclamation marks.

### Numbers and units

- Numerals for counts: "2 frames". "n of N" for progress ("Aligning 12 of 46"), never "12/46". The one slash form is the focus rail's readout, "18/46", a place along the rail rather than progress, kept short beside it.
- A space before units ("45 MP", "250 MB", "30 px", "12 s", "4 min", "24 fps"), none before `%` or `°` ("80%", "0.5°").
- Dimensions with a spaced `×` and no thousands separator: "8192 × 5464". Magnification unspaced: "0.5×".
- Ranges in sentences: "from 1 to 10"; never Rust's `{lo}..={hi}`.
- Sizes in KB, MB or GB, never a byte count.
- Durations with spaced units: "12.3 s", "4 min 10 s", "about 6 s left". Rust's `{:.1?}` prints "12.3s": format the number and add the unit.
- A progress line needs no noun when its verb names the thing ("Copying 3 of 12", "Aligning 12 of 46"); a count that stands alone does ("44 of 46 frames").

### Keys and menu paths

- Mac symbols in the order ⌃⌥⇧⌘, no plus: "⇧⌘E", "⌘-click", "⇧-click". Windows and Linux strings: "Ctrl+Shift+E", "Ctrl-click". Named keys: Esc, Space, Tab, Delete. A key after a name goes in brackets: "Export (⌘E)".
- Menu paths with a spaced ` ▸ `: "Settings ▸ Advanced ▸ GPU". Not `→`, `›` or `>`.

### How the reader is addressed

Second person, or the user's object as subject: "Your frames are only read", "1 frame can't be found". The imperative says what to do. When the app acts, Deptheos is the subject: "Deptheos left out 2 frames". Never we, our or us. Questions only in alert titles that ask for a decision.

### Contractions

Contract negatives: can't, don't, doesn't, didn't, isn't, wasn't, couldn't. Keep "not" whole only where it carries the stress ("Your frames are not changed"). No other contractions.

### Words to avoid

please, successfully, simply, "just" as a softener, e.g. (write "such as" or "for example"), i.e., etc., via, "Note:". In the apps also avoid words that name code: worker, thread, panic, buffer, texture, blob, manifest, pass, state, id, callback. ("The GPU engine" is the site's own phrase and may stay.)

### Fallback names for unnamed things

- A stack: the user's name, else its first frame's folder; with neither, "Untitled stack". A render: "#4", and "#1" when it is the stack's only one; never "Render 4" on screen, and never its method.
- An exported file: `{stack}-{render}` on both apps, "stack" when the name cleans to nothing ("stack-4.tif"). The pattern is a preference both apps share (ADR-150), made into a name by `deptheos_app::library::export::file_stem`.
- A turntable with no Bluetooth name: "unnamed turntable", not "(unnamed)".

### VoiceOver and accessibility labels

A label names the control in sentence case with no full stop and without its kind ("Focus peaking", not "Focus peaking button"). A switch keeps one label in both states and reports On or Off as its value. A label says enough to tell siblings apart: a render's tile is "Render 4, Pyramid, starred", not "#4", and the edited base gives "Edited" as its value; what the result is compared with reads "Comparing with render 3"; the batch card's menu is "Batch options", not "Batch". A hint, where one is needed, is one short sentence with a full stop.

## 4. Error and notice pattern

1. **Lead**: the user's object as the subject; past tense for an event, present for a standing limit. "Stack #5 didn't finish." "The Library folder couldn't be opened." "A RAW render and a JPEG render can't be compared." Sentence case, full stop.
2. **Reason**, when it helps, as its own sentence in the user's terms.
3. **What to do**, in the imperative.
4. **Details**: what a lower layer said goes to Copy Details and the log, not the visible line, unless it was written for users.

Each layer adds a sentence or nothing. It never prefixes "X failed:" to the error below it, so "alignment failed: ECC alignment failed: …" can't appear. Each sentence carries its own full stop, and they are joined through one message (`sentences` in `deptheos_app.ftl`, by `deptheos_app::words::sentences`), so the joining code adds no punctuation of its own. Splitting the lead from its reason ("Stack #5 didn't finish. {reason}") only reads right once the reasons below are sentences, so the two changes go in together.

A reason that is also shown inside a notice, such as why a frame was left out ("Left out IMG_0412.CR3 (…)"), is written as a lower-case clause about the frame with no full stop: "doesn't look enough like the frame before it to line up; try Grid search". Numbers a photographer can't use (a correlation, a matrix condition) go to the log. Don't open with "Could not …", "Failed to …", "Error:" or "… failed:". A fault the user can't fix (a panic, a lost thread, a state the code didn't expect) gets one fixed message, "Deptheos ran into an unexpected problem and stopped.", never "fault", "crash" or "error", with the raw text in Copy Details where the message has that button. Otherwise the message says how to reach the details: on the desktop its notice offers Save Diagnostics… and Show Log Folder, and the sentence names Help ▸ Save Diagnostics… and the support address to email it to (diagnostics never go on a public issue), so it reads right where it has no buttons. "Help ▸ Report a Problem…" exists only on the desktop; iOS says "Send Diagnostics… in the Library's Settings". Never shown: type, state or function names, `{:?}` output, `..=` ranges, GPU format names, "panicked", byte counts, hex codes, and the names of the Library folder's own files ("Part of the Library folder can't be found", not `"render.json" can't be found`). A file or folder the system won't let the app open "isn't allowed"; it is never called missing, and the desktop says where to allow it (on macOS, System Settings ▸ Privacy & Security ▸ Files & Folders).

**Where the text lives.** An engine error is a typed value with the facts in its fields (which frame, which sizes, which limit), and its own text is technical, for the log. One function in `deptheos_app` turns each error into the user's sentences for the desktop, iOS and the command line. An error it has no sentence for is an internal fault and gets the fixed message, so a new engine error can't reach the screen unworded. The engine crates hold no text written for users, and when the strings move to catalogs each sentence is already a key with values.

The command line keeps its own form for the lines it writes itself, because clap prints part of each: `error: ` then a lower-case message with no closing full stop, the option in its literal form (`--smoothness`), and the same terms as the apps. An engine error is the exception: its sentences follow `error: ` as the apps show them, capitals and full stops included, so one wording serves all three. An engine message never names an option; the command line layer adds it.

| Before | After |
|---|---|
| The stack didn't finish: alignment failed: ECC alignment failed: {reason} | The stack didn't finish. Frame 23 couldn't be lined up with the frame before it. Try Grid search in the inspector, or exclude the frame. |
| stacking panicked: {msg} | Deptheos ran into an unexpected problem and stopped. Click Copy Details, then choose Help ▸ Report a Problem… and paste them into the report. |
| Save failed: {message} | #4 couldn't be exported to "Desktop". {reason} |
| {width}x{height} frame needs a {bytes}-byte {what} buffer; GPU limit is {limit} | Frames of 8192 × 5464 are too large for this GPU. Stack smaller frames, or choose another GPU in Settings ▸ Advanced. |
| this GPU cannot store R8Unorm; use map_format() | This GPU lacks a feature stacking needs. Choose another GPU in Settings ▸ Advanced. |
| pyramid smoothness {} outside {lo}..={hi} | Smoothness must be from 1 to 10. |
| frame is {}x{} but the stack is {}x{}. Every frame must match… | "IMG_0412.CR3" is 6000 × 4000, but the stack's first frame is 8192 × 5464. Every frame must be the size of the first. Check that all come from the same camera and crop mode. |

Notices follow the same lead: "Left out 2 frames: IMG_0412, IMG_0413", "Stacking canceled. Every render already made is kept."

## 5. Counts and plurals

Every count has a singular and a plural form, chosen by the count. Never "frame(s)", never "1 frames", and never a noun built by appending "s". In the string catalogs each count becomes a plural variant (`one` and `other` in English; other languages need more), so write the whole phrase in each variant, not a noun slotted into a sentence.

- Number first, noun after: "1 frame", "46 frames".
- The verb agrees inside the same variant: "1 frame is missing." / "3 frames are missing." Don't stitch "frame is" / "frames are" fragments.
- "n of N" takes N's noun: "44 of 46 frames". Where N can be 1, show "1 frame" instead of "1 of 1 frame".
- Zero gets its own words where it shows: "No renders", not "0 renders". A sentence built around a count needs a zero variant of the whole sentence, not "No frames" slotted in.
- How the working version stands against a render is the engine's words (`deptheos_app::library::stacks`; `changes…`, `differences-from…` and `same-as-render` in `deptheos_app.ftl`), the same on both apps. Against its base it has *changes*: tiles and list rows give the total ("3 changes since #4"), the base's own tile, whose number already names it, gives "3 changes", and the change line beside Revert says what changed: "2 changes to the frames since #4", "1 change to the settings since #4", "1 change to the settings and 2 to the frames since #4". Against any other render shown it has *differences*, since nothing was changed from that render: "1 difference from #3 in the settings", "2 differences from #3 in the frames", "1 difference from #3 in the settings and 2 in the frames", and "Same frames and settings as #3" when there are none. A change or difference in the frames may be the RAW or JPEG choice or the frame order, so no form counts frames ("2 frames changed", "2 frames differ"). Against the base, nothing shows when nothing changed.
- Alert titles name the count: "Delete 3 renders?". One render is named, with its stack: "Delete bee #4?". Where the menu item knows the count it carries it on both apps ("Delete 3 Other Renders…").

On iOS the strings live in `ios/DeptheosMobile/Localizable.xcstrings` (and the permission prompts in `InfoPlist.xcstrings`), each with a comment that says where it shows and what each placeholder holds. A count is one entry that varies by plural, with a `zero` variant where the app shows words for zero; a sentence with two counts, or a count and a name, varies through a substitution. Text the engine or the system words (errors, the method's description, batch summaries) is not in the catalog: it is shown as it comes. `xcodebuild -exportLocalizations -project ios/DeptheosMobile.xcodeproj -localizationPath <folder> -exportLanguage en` brings the catalog up to date with the code: a build alone does not.

## 6. Desktop and iOS: what must match and what may differ

**Must match**: every term in section 2; method, slider and setting names; commands both apps have ("Revert to #3" without a `…`, "Render Info", "Compare with Result", "Delete Failed Stack…" with its `…`, "Unstar"); spelling; apostrophes; the middle dot; units; the error pattern; plurals; exported file names. One behavior gets one sentence where the screen allows it.

**May differ**, because the platform does:

- iOS navigation titles, section headers and empty-state titles in Title Case.
- iOS system words: Share…, Save to Files…, Photos, Files, iCloud Drive, the Settings app (the system's), Split View, Slide Over.
- "tap" and "touch and hold" on iOS; "click" and "right-click" on the desktop.
- "Settings…" in the macOS app menu and in ≡ on Windows and Linux; on iOS, Settings is a sheet opened from the Library.
- Finder, Explorer or file manager by desktop platform; "Ctrl" for "⌘" in Windows and Linux strings.
- iOS "can't be reached" and "Not Connected" for frames on a drive or server that isn't attached; the desktop has only "can't be found".
- iOS "Send Diagnostics…" (in Settings, it opens the share sheet, `AboutView.swift`) against the desktop's "Save Diagnostics…" (saves a file).
- VoiceOver may read "Render 4" where the screen shows "#4".
- Shorter labels on iPhone where the desktop's won't fit.

**Must not differ**: what a string says the app does.
