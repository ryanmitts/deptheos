# Deptheos design language

The brand's colors, corners and type live in one file, `tokens.json`. The desktop and iOS apps' themes are generated from it, so they stay in step. The website sets its own (below).

## Rules

- **The photo is the loudest thing on screen.** Chrome stays quiet: cool grays, one accent, no decoration.
- **One accent, for actions only.** The blue from the app icon marks what you can click, what is selected, and what has focus. It is not used for decoration or headings.
- **The photo backdrop is neutral.** Surfaces may lean slightly cool, but the area behind a photo (`viewer`) is a plain dark gray in both modes, so colors read true.
- **Chrome over the photo is one recipe.** What the app draws over a photo (tile pills, the viewer's tags, Retouch's chips, the drop targets, the Library's preview) is the `hud` fill, edge and text, the same in light and dark mode, with the theme's corner and one shadow (`overlay_surface` in the desktop's `look.rs`). A floating bar or card built from the kit's controls (the view bar, the progress card) has the same edge, corner and shadow on the theme's surface, because the kit draws its controls in the theme's colors.
- **Status colors mean status.** Green is done, amber needs a look, red is an error. They are not used elsewhere.
- **System fonts in the apps.** SF on Apple platforms, Segoe UI on Windows. Numbers that line up use the system font's tabular digits (the `tnum` feature), not a monospace font. The website sets its own type (below).
- **Two corner sizes.** 6 px for most things, 8 px for dialogs and cards. Small details (inline code, labels on photos) may go down to 4 px.
- **A quiet website.** Real photos and measured numbers are the visuals. Links are underlined. No stock gradients; the one place a gradient belongs is a depth map, where it shows data (and the app icon's own fill, in the logo).
- **iOS stays native.** Only the accent is ours. Grays, fonts, lists and controls are Apple's system ones. In dark mode iOS gets a deeper blue (`ios-dark`) than the other platforms, because iOS puts white labels on the tint.
- **Readable everywhere.** Text is at least 4.5:1 against what it sits on, and control edges (inputs, checkboxes, off switches) at least 3:1, in both modes.

## The website's own layer

The website does not read `tokens.json`. Its palette, type and spacing are in `web/src/site/site-tokens.css`, a hand-written file that only the website reads, so nothing there reaches the desktop or iOS app:

- **Palette.** The same cool grays and the app icon's blue (`#1f57a3` in light), tuned for long reading: a background a shade off the panels, body text a step softer than headings (`--body`), a stronger line for table heads (`--line-strong`), an accent hover and tint. Dark mode keeps the photo backdrop neutral.
- **Type.** IBM Plex Sans for text and IBM Plex Mono for data (captions in camera-data form, labels, numbers, figure readouts), both OFL and self-hosted from `@fontsource`, with the system stack as fallback. A scale of about 1.25 on 17 px.
- **Spacing and corners.** A 4 px spacing scale, a 680 px text column, a 1200 px page; cards and figures use 10 px corners.
- **The logo.** The app icon's glyph on its blue tile, from `DeptheosAppIcon.icon` (see `web/README.md`).

Change an app value in `tokens.json` and a website value in `site-tokens.css`. A change to the brand itself (the accent) goes in both.

## Changing a value

1. Edit `design/tokens.json`.
2. Run `node script/tokens.mjs`.
3. Commit the token file and the generated files together. `node script/tokens.mjs --check` fails if they differ.

A color is `#rrggbb`, or `#rrggbbaa` where it is see-through (only colors the desktop paints itself, in `brand.rs`). The generator refuses a `viewer` or `hud*` token that differs between light and dark; `node --test script/` runs its tests.

## Generated files

| File | Platform | What it holds |
|---|---|---|
| `ios/DeptheosMobile/Assets.xcassets/AccentColor.colorset` | iOS | The tint, light and dark. Set as the app's global accent in the Xcode project. |
| `ios/DeptheosMobile/Assets.xcassets/Viewer.colorset` | iOS | The photo backdrop, `Color.viewer` in Swift. |
| `ios/DeptheosMobile/Assets.xcassets/AccentFill.colorset` | iOS | The one primary button fill (`Stack`, `Done`), `Color.accentFill` in Swift. |
| `ios/DeptheosMobile/Assets.xcassets/Edited.colorset` | iOS | The dot that marks something edited, `Color.edited` in Swift. |
| `ios/DeptheosMobile/Assets.xcassets/Hud*.colorset` | iOS | Chrome over the photo (`Color.hud`, `Color.hudBorder`, `Color.hudText`). |
| `crates/deptheos_gpui/themes/deptheos.json` | Desktop | gpui-kit light and dark themes, loaded at startup. The generator maps each token to gpui-kit's keys. |
| `crates/deptheos_gpui/src/brand.rs` | Desktop | Colors the app paints itself (viewer backdrop, marks and chrome over the photo). |

## Tokens

| Token | Used for |
|---|---|
| `background` | Page and window |
| `surface` | Panels, cards, popovers, desktop sidebar |
| `surface-sunken` | Inputs, inline code, panels inside the sidebar |
| `surface-hover` / `surface-active` | Hover / pressed on neutral controls |
| `text` / `text-muted` | Body text / captions and labels |
| `text-segment` | Labels of a segmented control's unselected segments |
| `line` | Borders and dividers |
| `control-border` | Edges of inputs, checkboxes and off switches |
| `accent` / `on-accent` | Actions, links, selection, focus / text on accent fills |
| `accent-fill` / `on-accent-fill` | The one primary button per screen: Stack in Stacking, Done in Retouch, the welcome's Add Frames… on an empty Library (the Library's title bar has none) / its text |
| `edited` | The dot that marks something edited |
| `success` / `warning` / `danger` | Status |
| `viewer` | Photo backdrop |
| `hud` / `hud-border` / `hud-text` | Chrome over the photo: its translucent fill, its edge, its text. The same in both modes |

Name new tokens by what they are for, not by their color, so a value can change without touching the code that uses it.

## Segmented controls

gpui-kit draws the selected segment of its segmented control in the window background color, which in dark mode is darker than the track. The desktop draws its own (`Segmented` in `crates/deptheos_gpui/src/form.rs`) for Method, the RAW + JPEG pairs choice, Brush / Eraser and the Export sheet's format: the selected segment is `surface` with a small shadow on a `surface-sunken` track, in both modes, with `text` on it and `text-segment` on the others.

## Icons: one glyph per control on both apps

The desktop draws Lucide icons through gpui-kit (`IconName`); iOS draws SF Symbols. Neither set has the other's glyphs, so each control is given the pair that looks most alike, and the two apps read as one. **A new control picks the pair that looks alike, and this table is updated with it.** A desktop icon outside gpui-kit's default set is added to `ExtraIcons` in `crates/deptheos_gpui/src/main.rs` before it can be drawn; one of the app's own, drawn in `design/icons/` on Lucide's grid, is an `AppIcon` in the same file. "—" means that app has no such control, or shows it as words only.

| Control | iOS (SF Symbol) | Desktop (`IconName`) | Notes |
|---|---|---|---|
| Stack (the command), and "a stack" | `square.3.layers.3d` | `Layers` | The command and a stack only: a render has its own glyph. |
| A render (tiles, the Renders list's rows, the view bar's home control and its menu's items, other menu items that list renders, Compare With's renders, the Library's count, No Result Yet) | `square.3.layers.3d.top.filled` | `Render` (`design/icons/render.svg`, an `AppIcon`) | The Stack glyph with its top layer filled; never on a sentence that already names the render. Tinted as the text beside it, never the accent. |
| Methods (Pyramid, Depth, Wavelet) | — | — | No glyphs, by decision: the Method picker and the summaries are words. |
| Stack Settings (the iPhone's button) | `design/icons/stack-settings.svg` (a custom glyph) | — | The desktop's settings are the inspector, always there. |
| Frames (the iPhone's button) | `photo.on.rectangle.angled` | `Images` | The desktop's is Retouch's Paint From: Frames, in the view bar and its menu. |
| The two columns: Frames, inspector | `sidebar.leading`, `sidebar.trailing` | `PanelLeft`, `PanelRight` | |
| Retouch | `paintbrush.pointed` | `Brush` | |
| Export | `square.and.arrow.up` | `Share` | |
| Compare, and Split | `square.split.2x1` | `SquareSplitHorizontal` | Compare turns on the layout used last; Split is the layout. On iOS the view menu's button shows the symbol of what is on (the layout on screen, else Depth Map's, else Contribution's), lit, and Compare's, unlit, while nothing is; the desktop's Compare keeps Split's. |
| A menu beside a control (the desktop's Compare layouts and zoom presets, iOS Retouch's home control and layout), or in a control's own label where a tap opens it (iOS's home control with other renders) | `chevron.down` | `ChevronDown` | The body of a split button acts; the chevron opens its menu. On iOS each part has its own glass. |
| Compare With (iOS's view menu) | — | — | A submenu of words: the frame on the rail, then the renders with their glyph. |
| Side by Side | `rectangle.split.2x1` | `Columns2` | |
| Top and Bottom | `rectangle.split.1x2` | `Rows2` | Also a Retouch layout on iOS; the desktop's Retouch has none. |
| Flicker | `circle.dotted.and.circle` | `Blend` | Two overlapping shapes, unlike Flip's arrows. |
| iOS's Flip, the desktop's Swap | `arrow.left.arrow.right` | `ArrowLeftRight` | The same glyph for two actions: Flip switches Flicker's image; Swap opens the compared render and compares with the one that was open. |
| Result (the Painted button in Retouch), and Compare's Off (the first item of iOS's view menu) | `square` | `Square` | |
| Depth Map | `mountain.2` | `Mountain` | A button in the desktop's view bar and iOS Retouch's deck; an item of iOS Stacking's view menu, whose button takes the symbol while it is on (ADR-149). |
| Contribution | `circle.dashed.inset.filled` | `Highlighter` | The same places as Depth Map, on only while a frame is on screen. |
| The focus rail | — | — | No glyph: ticks drawn in the muted text color, the frame on screen in the accent, frames left out in the warning color, and the `edited` dot over frames painted from. |
| Focus Mode | — | `PanelBottom` | The desktop's only: iOS hides its controls with a tap or Hide Controls. |
| Zoom In, Zoom Out | — (pinch; Zoom to Fit is words, except in Retouch's toolbar, at every width: `arrow.up.left.and.down.right.magnifyingglass`) | `Plus`, `Minus` | |
| Previous frame, next frame | — | — | No arrows on either app: the focus rail, ← and → step the frames (ADR-149, ADR-163). |
| Back to the Library | `chevron.backward` | `ChevronLeft`, with the word "Library" | On iOS the symbol alone, in the bar's cancellation placement; its title is for the overflow menu, VoiceOver and the pointer. |
| Close (an iOS sheet) | the system's (`Button(role: .close)`) | — | In the sheet's cancellation placement, as every sheet's. |
| Undo, Redo | `arrow.uturn.backward`, `arrow.uturn.forward` | `Undo2`, `Redo2` | |
| Revert | `arrow.counterclockwise` | — | Not Undo's glyph. The desktop's Revert is a menu item. |
| Rename | `pencil` | — | A menu item on the desktop. |
| Star, Unstar | `star`, `star.slash`; a starred tile shows `star.fill` | — | The desktop writes ★ in the render's label. |
| Delete | `trash` | — | A menu item on the desktop. |
| Add | `plus` | `Plus` | |
| Camera (Take Burst) | `camera` | — | The desktop has no camera; its RAW settings page uses `Camera`. |
| A frame left out of the stack | `exclamationmark.triangle.fill` | `TriangleAlert` | In the warning color. iOS's badge uses it for a place that isn't connected ("T7 SSD Not Connected"). |
| Where the frames are (iOS's Frames head) | `externaldrive`, `server.rack`, `icloud` | — | A drive, a server, iCloud Drive. The desktop reads frames where they are and names no place. |
| Copy Frames into Deptheos | `square.and.arrow.down.on.square` | — | In the badge's menu, or the Frames list's commands. |
| Locate… | `folder` | — | Words on the desktop. |
| Open (a Library stack) | `arrow.up.forward.square` | `ArrowRight` | |
| More (an overflow menu) | — (the bar's own overflow menu; before iOS 27 the Frames list's commands are a `checklist` menu) | `Ellipsis` | On iOS the ellipsis is the system's alone: the app adds to the bar's overflow menu (`ToolbarOverflowMenu`) or names a menu by its domain. |
| Settings (the app's) | `gearshape` | — | iOS: the Library's button that opens Settings. The desktop's is Settings… in the app menu (⌘,), as words. |
| Brush settings (Retouch's toolbar at compact width) | `slider.horizontal.3` | — | The desktop's brush is in the inspector, as on the iPad. |
| Show the source (Retouch) | `eye`, `eye.fill` while shown | — | In iOS Retouch's deck: a tap shows it or the layout again, a touch and hold shows it while held. The desktop holds `\`. |
| Sharpest Frame (iOS Retouch) | `eyedropper` | — | The desktop's is ⌥-click, with no button. |
| Brush, Eraser (iOS Retouch's deck at compact width) | `paintbrush.pointed`, `eraser` | — | The brush shares Retouch's glyph. The desktop's are words in the inspector's segmented control. |
