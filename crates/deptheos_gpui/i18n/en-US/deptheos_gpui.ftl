# The desktop app's own strings, in American English: its menus, windows,
# panels, dialogs and notices (ADR-161). Each message is read through
# `say!` in crates/deptheos_gpui/src. What the shared layer says (errors,
# notices, batch lines, method names) is in
# crates/deptheos_app/i18n/en-US/deptheos_app.ftl. The wording follows
# docs/terms-and-usage.md.
#
# One message per label or whole sentence. Arguments are finished text
# (names already quoted, sizes and dimensions already formatted) except a
# count that chooses a plural, which is a number. Key names ("⌘E") are
# arguments, written for the platform.

-app-name = Deptheos

## Across screens: the same control or the same count wherever it shows.

button-cancel = Cancel
# A button that opens a file or folder picker.
button-choose = Choose…

# A percentage: the zoom, a slider's value, a stack's progress. $value:
# the number, formatted ("80").
unit-percent = { $value }%
# A length in pixels. $value: the number, formatted ("30").
unit-px = { $value } px

# A count of frames. $n: how many.
frames-count = { $n ->
    [one] 1 frame
   *[other] { $n } frames
}
# A count of renders. $n: how many.
renders-count = { $n ->
    [one] 1 render
   *[other] { $n } renders
}
# A count of stacks. $n: how many.
stacks-count = { $n ->
    [one] 1 stack
   *[other] { $n } stacks
}

## The GPU start (main.rs): the window shown instead of the app when the
## engine can't start, and the notice when the chosen GPU couldn't.

# The window's title, and the title of the system dialog when the window
# can't open.
window-title = { -app-name }
# What the startup error says after an engine fault's reason. $reason: the
# fault's sentence.
gpu-fault-reason = { $reason } The details are in the log.
# The startup error window's text. $why: why the GPU couldn't start, one
# or more sentences.
gpu-start-failed = { -app-name } couldn't start its GPU engine. { $why } Choose another GPU, or Software rendering, in Settings ▸ Advanced.
# The notice when the GPU chosen in Settings couldn't start. $gpu: one of
# the gpu-subject messages; $reason: why it couldn't start.
gpu-fallback-notice = { $gpu } couldn't start, so { -app-name } is using the fastest GPU it found. You can choose another in Settings ▸ Advanced.

    { $reason }
# The notice at launch when the settings file was there but couldn't be
# read, and was set aside. $path: where the old file was kept.
settings-reset-notice-kept = Your settings couldn't be read, so { -app-name } is using the defaults. The old file was kept as { $path }.
# The same when the old file couldn't be set aside.
settings-reset-notice = Your settings couldn't be read, so { -app-name } is using the defaults.
# A GPU choice as a sentence's subject.
gpu-subject-automatic = The fastest GPU
gpu-subject-software = The software renderer
# $name: the GPU's name.
gpu-subject-named = The chosen GPU ({ $name })
# Under the startup error. $path: the settings file's path.
settings-saved-in = Settings are saved in { $path }.
# The startup error window's button.
open-settings = Open Settings
# The system dialog when the main window can't open. $error: the system's
# reason.
window-failed = { -app-name } couldn't open its window.

    { $error }

## The start window (start.rs): shown in place of the main window when the
## Library won't open. Why is the shared layer's sentence.

# Its heading.
start-title = { -app-name } can't open its Library
# Under the heading. $folder: the Library folder's path.
start-folder = Library folder: { $folder }
# Why, when there is no folder for the Library at all.
start-no-folder = There's no folder to keep the Library in. Choose a folder for it.
start-try-again = Try Again
# Try Again's words while it tries.
start-trying = Opening…
start-choose = Choose Another Library…
start-quit = Quit
# First, when a newer version made the Library: opens the website's
# download page.
start-get-latest = Get the Latest Version

## The main window's title (app.rs), which the Window menu, Mission
## Control and the window switcher show.

# What is open, then the app's name. $open: the title bar's name for what
# is open: the stack's name, toolbar-new-stack-title or
# toolbar-library-title.
window-title-open = { $open } — { -app-name }

## The main window (app.rs): a stack's end, notices, the drop overlay.

app-stacking-canceled = Stacking canceled
# The Export… button on a notice: a stack's whose render wasn't kept,
# and an export's that failed.
app-stacked-export = Export…
# The button on a failed stack's notice in the Library: it shows the
# failure in Stacking's inspector.
app-show-in-stacking = Show in Stacking
# A stack's end. $seconds: how long it took, to a tenth ("1.2").
app-stacked-in = Stacked in { $seconds } s
# $seconds: how long it took, to a tenth ("1.2"); $left_out: the sentence
# saying which frames were left out, with no period.
app-stacked-in-left-out = Stacked in { $seconds } s. { $left_out }.
# A stack's end, its render kept. $seconds: how long it took, to a tenth
# ("1.2"); $number: the render's number.
app-stacked-in-numbered = Stacked in { $seconds } s · #{ $number }
# $seconds: how long it took, to a tenth ("1.2"); $number: the render's
# number; $left_out: the sentence saying which frames were left out,
# with no period.
app-stacked-in-numbered-left-out = Stacked in { $seconds } s · #{ $number }. { $left_out }.
# $seconds: how long it took, to a tenth ("1.2").
app-stacked-in-no-thumbnail = Stacked in { $seconds } s. It wasn't kept in the Library, because its thumbnail couldn't be made. Export it to keep it.
# $seconds: how long it took, to a tenth ("1.2"); $left_out: the sentence
# saying which frames were left out, with no period.
app-stacked-in-left-out-no-thumbnail = Stacked in { $seconds } s. { $left_out }. It wasn't kept in the Library, because its thumbnail couldn't be made. Export it to keep it.
# The lead of an export's error. $folder: the folder's name.
app-export-failed-to = The render couldn't be exported to "{ $folder }".
app-export-failed = The render couldn't be exported.
# $reason: why, in sentences.
app-frame-not-shown = The frame couldn't be shown. { $reason }
# $reason: why, in sentences.
app-compare-not-shown = The render couldn't be shown for comparing. { $reason }
# Over the window while files are dragged in, while a stack runs.
app-drop-while-stack = Frames can be added once the stack finishes
# The same while a batch runs.
app-drop-while-batch = Frames can be added once the batch finishes
# The same while a grow sequence is written.
app-drop-while-sequence = Frames can be added once the sequence finishes
app-drop-frames = Drop frames or a folder to add them
# The drop overlay's two targets over the Library while two or more
# folders are dragged in: their frames as one stack, or a stack for each
# folder, set up in the New Batch dialog.
app-drop-folders-one-stack = One Stack
# Under One Stack. $n: how many folders.
app-drop-folders-one-stack-detail = { $n ->
    [one] The folder's frames, stacked together
   *[other] The { $n } folders' frames, stacked together
}
app-drop-folders-batch = A Stack per Folder (Batch…)
# Under A Stack per Folder (Batch…). $n: how many folders.
app-drop-folders-batch-detail = { $n ->
    [one] 1 stack, set up in New Batch
   *[other] { $n } stacks, set up in New Batch
}
# The name of a stack with frames but no name yet.
app-untitled-stack = Untitled stack

## The viewer (app/view.rs, viewer/): the zoom field, the compare pane's
## label and loading.

# The zoom field when the photo fits the window.
viewer-zoom-fit = Fit
# The compare pane of the stacked result.
viewer-result = Result
viewer-color-management-on = Color management on
viewer-color-management-off = Color management off: the window shows the pictures' values unconverted. Wide-gamut pictures change the next time they are shown.
# While a frame loads. $name: the frame's file name.
viewer-loading = Loading { $name }…
# While the depth map's picture, let go of while the map was off, is made
# again.
viewer-loading-depth-map = Loading the depth map…

## The menu bar (app/actions.rs). Items name what they will do; an item
## whose words flip with the state has a message for each.

menu-app = { -app-name }
menu-about = About { -app-name }
menu-check-for-updates = Check for Updates…
menu-settings = Settings…
menu-install-command-line-tool = Install Command Line Tool…
menu-services = Services
menu-hide-app = Hide { -app-name }
menu-hide-others = Hide Others
menu-show-all = Show All
menu-quit = Quit { -app-name }

menu-file = File
menu-new-stack = New Stack
menu-new-batch = New Batch…
menu-add-frames = Add Frames…
menu-add-folder = Add Folder…
menu-export = Export…
menu-export-again = Export Again
menu-export-grow-sequence = Export Grow Sequence…
menu-show-frames-in-finder = Show Frames in Finder
menu-close-window = Close Window

menu-edit = Edit
menu-undo = Undo
# $step: what Undo takes back ("Paint").
menu-undo-step = Undo { $step }
menu-redo = Redo
# $step: what Redo puts back ("Paint").
menu-redo-step = Redo { $step }
menu-cut = Cut
menu-copy = Copy
menu-paste = Paste
menu-select-all-frames = Select All Frames
menu-rename-stack = Rename Stack…
menu-rename-render = Rename Render…
menu-remove-all-frames = Remove All Frames

menu-view = View
menu-result = Result
# The View menu's first layout in Retouch: the painted result alone.
menu-painted = Painted
menu-split = Split
menu-side-by-side = Side by Side
menu-top-and-bottom = Top and Bottom
menu-flicker = Flicker
menu-swap-sides = Swap Sides
menu-show-depth-map = Show Depth Map
menu-hide-depth-map = Hide Depth Map
menu-show-contribution = Show Contribution
menu-hide-contribution = Hide Contribution
menu-zoom-in = Zoom In
menu-zoom-out = Zoom Out
menu-zoom-to-fit = Zoom to Fit
menu-actual-size = Actual Size
menu-stacking = Stacking
menu-library = Library
menu-show-frames = Show Frames
menu-hide-frames = Hide Frames
menu-show-inspector = Show Inspector
menu-hide-inspector = Hide Inspector
menu-focus-mode = Focus Mode
menu-enter-full-screen = Enter Full Screen
menu-exit-full-screen = Exit Full Screen

menu-stack = Stack
# The Stack menu's item that runs a stack.
menu-run-stack = Stack
# The Stack menu's Cancel item (⌘.), named for what runs; disabled while
# nothing runs.
menu-cancel-stack = Cancel Stack
menu-cancel-batch = Cancel Batch
menu-cancel-sequence = Cancel Sequence
# With no render to go back to, disabled.
menu-revert = Revert
# $number: the render shown, whose frames and settings the working
# version takes.
menu-revert-to = Revert to #{ $number }
# The record of the render shown, read-only.
menu-render-info = Render Info
menu-retouch = Retouch
menu-done-retouching = Done Retouching
menu-star-render = Star Render
menu-unstar-render = Unstar Render
menu-exclude-selected-frames = Exclude Selected Frames
menu-include-selected-frames = Include Selected Frames
menu-include-only-selected-frames = Include Only Selected Frames
menu-include-all-frames = Include All Frames
menu-previous-frame = Previous Frame
menu-next-frame = Next Frame

menu-window = Window
menu-minimize = Minimize
menu-zoom = Zoom
menu-bring-all-to-front = Bring All to Front

menu-help = Help
menu-app-help = { -app-name } Help
menu-keyboard-shortcuts = Keyboard Shortcuts
menu-choosing-a-method = Choosing a Method
# Adds a sample stack the app carries to a new stack, which waits for
# Stack: a submenu of them by name (Hornet, Circuit Board) when the app
# carries two, the one alone when it carries one; in a build without one,
# opens the website's sample stacks.
menu-try-sample = Try a Sample Stack
menu-report-a-problem = Report a Problem…
menu-save-diagnostics = Save Diagnostics…
menu-show-log-folder = Show Log Folder
menu-release-notes = Release Notes

## The ≡ menu (app/window_menu.rs), off macOS: the bar's menus, with these
## in place of the items macOS has elsewhere.

window-menu-tooltip = Menu (Alt)
menu-show-frames-in-explorer = Show Frames in Explorer
menu-show-frames-in-file-manager = Show Frames in File Manager
menu-exit = Exit

## Help ▸ Keyboard Shortcuts and Help ▸ Choosing a Method (app/menu.rs).
## A shortcut's keys are shown beside its words, as written in the code.

shortcuts-title = Keyboard Shortcuts
shortcuts-stacking = Stacking
shortcut-stack = Stack
shortcut-add = Add Frames… / Add Folder…
shortcut-export = Export… / Export Again
shortcut-render-info = Render Info: the record of the render shown
shortcut-new-batch = New Batch…
shortcut-exclude = Exclude or include the selected frames
# Delete or Backspace with the Frames list focused.
shortcut-exclude-delete = In the Frames list: exclude the selected frames
shortcut-star = Star or unstar the render shown
shortcut-cancel = Cancel the stack or batch
shortcuts-viewing = Viewing
shortcut-layouts = Result, Split, Side by Side, Top and Bottom
shortcut-flicker = Flicker between the two sides
shortcut-swap-sides = Swap Sides
shortcut-map = Depth Map / Contribution
shortcut-frames = Previous Frame / Next Frame
# Beside the Option-drag (Alt-drag) shortcut: a drag along the focus rail
# with the key held moves through the frames one at a time.
shortcut-fine-scrub = Along the focus rail, one frame at a time
shortcut-zoom-to-fit = Zoom to Fit
shortcut-actual-size = Actual Size
shortcut-fit-or-actual-size = Switch between Zoom to Fit and Actual Size
shortcut-zoom-in-out = Zoom In / Zoom Out, a step at a time
# A double-click on the picture.
shortcut-double-click = Double-click
shortcut-double-click-zoom = Switch between Zoom to Fit and Actual Size at the pointer
# Scrolling with a key held. $key: the modifier ("⌘").
shortcut-key-scroll = { $key }-scroll
shortcut-scroll-zoom = Zoom at the pointer
shortcut-stop-compare = Stop comparing
shortcut-focus-mode = Focus Mode
shortcut-screens = Stacking / Library
shortcut-panels = Frames / Inspector
shortcuts-retouch = Retouch
shortcut-retouch = Retouch / Done Retouching
shortcut-tools = Brush / Eraser
shortcut-brush-size = Brush size
shortcut-hardness = Hardness
shortcut-opacity = Opacity, 10% to 100%
shortcut-sharpest = Paint from the sharpest frame there
shortcut-see-source = See the source
shortcut-undo = Undo / Redo
# The key for See the source, held. $key: the key.
shortcut-key-held = { $key } (held)
# Alt-click. $key: the modifier ("⌥").
shortcut-key-click = { $key }-click
# A drag with a key held. $key: the key ("Space").
shortcut-key-drag = { $key }-drag
# Space-drag while retouching: it moves the picture and paints nothing.
shortcut-pan = Pan without painting
shortcuts-library = Library
shortcut-library-open = Open the selected stack
shortcut-library-move = Select the stack beside, above or below
shortcut-library-extend = Select the stacks up to the one beside, above or below
shortcut-library-preview = Show the selected stack large, or close it
shortcut-library-select-all = Select every stack
shortcut-library-search = Search the stacks
shortcut-library-delete = Move the selected stacks, or the render selected, to Recently Deleted
shortcut-library-deselect = Close the stack shown large, or select nothing

choosing-a-method-title = Choosing a Method
choosing-a-method-hint = Not sure? Stack with Pyramid, then again with Depth, and compare the two renders with Split.

## Days and ages (renders/labels.rs): the Library's day headings and the
## strip's tooltips.

# A day heading: the day itself.
age-today = Today
# A day heading: the day before.
age-yesterday = Yesterday
# When a render was made, in the Library's inspector. $day: one of the day
# headings, or a date ("Oct 28"); $time: the time of day ("2:32 PM").
age-day-time = { $day }, { $time }
# How long ago a render was made, under a minute.
age-just-now = just now
# $n: how many minutes ago.
age-minutes = { $n ->
    [one] 1 minute ago
   *[other] { $n } minutes ago
}
# $n: how many hours ago.
age-hours = { $n ->
    [one] 1 hour ago
   *[other] { $n } hours ago
}
# $n: how many days ago.
age-days = { $n ->
    [one] 1 day ago
   *[other] { $n } days ago
}
# $n: how many weeks ago.
age-weeks = { $n ->
    [one] 1 week ago
   *[other] { $n } weeks ago
}
# $n: how many months ago.
age-months = { $n ->
    [one] 1 month ago
   *[other] { $n } months ago
}
# $n: how many years ago.
age-years = { $n ->
    [one] 1 year ago
   *[other] { $n } years ago
}

## What a render stacked (renders/labels.rs): the parts of the strip
## tooltip's summary, joined by " · ".

# Some frames were left out. $stacked: how many it stacked; $total: how
# many were listed.
render-frames-of = { $total ->
    [one] { $stacked } of 1 frame
   *[other] { $stacked } of { $total } frames
}
# The render was stacked with Grid search alignment.
render-grid-search = Grid search
# The render keeps its uncropped edges.
render-uncropped = uncropped
# The render's frames were stacked where they lay, not aligned (Align
# Frames off, on the iOS app or with the CLI's --align none).
render-unaligned = not aligned

## The strip along the bottom of Stacking (renders/mod.rs, renders/tiles.rs).

renders-strip-heading = Renders
# Beside the heading, when the stack has no render.
renders-strip-none = None yet
renders-strip-saving = Saving…
# Under a tile while its render opens.
tile-opening = Opening…
# A tile's tooltip. $range: the first and last frames stacked
# ("IMG_0120–160"); $frames: what it stacked (frames, RAW, alignment);
# $age: how long ago it was made.
tile-tip = { $range } · { $frames } · { $age }
# A tile's tooltip with what was left out of it. $tip: the tooltip;
# $note: the sentence about the frames left out.
tile-tip-left-out =
    { $tip }
    { $note }
# A tile's tooltip in a narrow window, where the tile drops its settings
# line. $method: the stacking method; $settings: its settings; $tip: the
# rest of the tooltip.
tile-tip-compact =
    { $method } · { $settings }
    { $tip }
# Over the thumbnail of the reference pinned while comparing.
tile-tag-a = A
# The tooltip of the "A" tag.
tile-tag-a-tip = Compared render
# Over the thumbnail of the result on screen while comparing.
tile-tag-b = B
# The tooltip of the "B" tag.
tile-tag-b-tip = Shown result
# The pill over the thumbnail of the working version's base, once the
# working version has changed since it.
tile-edited = Edited
# Under the tile of the stack running.
tile-stacking = Stacking…

## A strip tile's right-click menu (renders/menus.rs), and the menus of
## the Library's render rows (renders/library.rs).

# Shows the render on screen; the frames and settings stay the working
# version's.
render-open = Open
# The render's record, read-only.
render-info = Render Info
# $number: this render, whose frames and settings the working version
# takes.
render-revert-to = Revert to #{ $number }
# In Render Info, when the render was stacked. $day: its date ("Nov 14,
# 2026"); $time: its time ("10:42 AM").
render-info-date = { $day } at { $time }
# In Render Info, for a starred render. $stacked: when it was stacked
# ("Stacked Nov 14, 2026 at 10:42 AM").
render-info-starred = { $stacked } ★
# Compares this render with the result on screen.
render-compare-mac = Compare with Result (⌘-click)
render-compare = Compare with Result (Ctrl-click)
render-export-grow-sequence = Export Grow Sequence…
render-rename = Rename Render…
render-star = Star
render-unstar = Unstar
render-delete = Delete Render
# $n: how many renders it would delete (0 when there are none).
render-delete-others = { $n ->
    [0] Delete Other Renders
    [one] Delete 1 Other Render
   *[other] Delete { $n } Other Renders
}
# Reveal a file, on macOS.
render-show-in-finder = Show in Finder
# Reveal a file, on Windows.
render-show-in-explorer = Show in Explorer
# Reveal a file, on Linux.
render-show-in-file-manager = Show in File Manager

## Saving, opening, renaming and deleting renders (renders/queue.rs,
## renders/mod.rs, renders/opening.rs, renders/tiles.rs): the leads of
## the notices when they fail. A delete's notice is the shared layer's,
## with Undo.

renders-list-failed = The Library folder couldn't be read.
render-open-failed = The render couldn't be opened.
render-rename-failed = The render's new name couldn't be saved.
render-stack-rename-failed = The stack's new name couldn't be saved.
render-save-failed = The render couldn't be saved.
render-star-failed = The render couldn't be starred.
render-unstar-failed = The render couldn't be unstarred.
render-failure-save-failed = The failed stack couldn't be kept in the Library.
# Starting a batch, its stacks couldn't be recorded to try again later.
render-unstacked-save-failed = The batch's stacks couldn't be kept in the Library to try again later.
# A batch ended, and its failed stacks couldn't be read again.
render-failures-read-failed = The batch's failed stacks couldn't be read again.
render-relink-failed = The renders couldn't be pointed at the new folder.
# At launch, looking for the folders of frames not where they were failed.
render-find-again-failed = The folders of frames that moved couldn't be looked for.
renders-delete-stacks-failed = Not every stack could be deleted.
renders-delete-stacks-unsure = The stacks may not all have been deleted.
renders-delete-failed = Not every render could be deleted.
renders-delete-unsure = The renders may not all have been deleted.
# Why a delete didn't start: something is running, which the Library
# doesn't know the kind of.
renders-delete-busy = Nothing was deleted, because a stack, batch or sequence is running. Delete again when it finishes.
# The same while a render is saved.
renders-delete-saving = Nothing was deleted, because a render is still being saved. Delete again when it is saved.
# The same while another delete runs.
renders-delete-deleting = Nothing was deleted, because another delete is still running. Delete again when it finishes.
# The button on a delete's notice that puts back what it deleted.
renders-deleted-undo = Undo

## The Library screen (renders/library.rs): its title bar's search, the
## left bar, the grid and its tiles. Its count line, Jump To's spans and a
## tile's badge are the engine's (library.rs).

# The Library's search field, when empty. It matches stack, render and
# batch names.
library-search = Search
# The left bar's heading over All stacks, Starred and Failed (shown in
# capitals).
library-show = Show
library-show-all = All stacks
library-show-starred = Starred
library-show-failed = Failed
library-delete-all-failed = Delete All Failed Stacks
# The left bar's last row, below the filters, and the title over its list
# (renders/library_deleted.rs).
library-recently-deleted = Recently Deleted
# Restores the records selected in Recently Deleted, as a button and in a
# record's menu.
library-deleted-restore = Restore
# Removes every record in Recently Deleted for good, after asking.
library-deleted-empty = Empty Recently Deleted…
# Recently Deleted's list while it is read.
library-deleted-reading = Reading Recently Deleted…
# Under a record's thumbnail. $days: its days left ("29 days left"); $size:
# what it takes, formatted ("610 MB").
library-deleted-detail = { $days } · { $size }
# The left bar's heading over the periods (shown in capitals).
library-jump-to = Jump to
# The grid, while the Library is read at launch.
library-reading = Reading the Library…
# The grid, when the Library couldn't be read (a notice says why).
library-unread = The Library couldn't be read.
# The grid, when there is no stack at all.
library-empty = No stacks yet. New Stack starts one, and each is kept here.
# The grid, when the search lets nothing through. $query: what was typed.
library-no-match-search = No stack, render or batch name has "{ $query }" in it. Clear the search to see every stack.
# The grid, when Starred lets nothing through.
library-no-match-starred = No starred renders. Star a render to keep it here.
# The grid, when Failed lets nothing through.
library-no-match-failed = No failed stacks. Choose All stacks to see the rest.
# The heading over the stacks with frames and no render.
library-unstacked-heading = Not stacked yet
# Over a tile's thumbnail: a batch stack that failed.
library-tile-failed = Failed
# Over a tile's thumbnail: a stack with frames and no render.
library-tile-unstacked = Not stacked yet
# Under a failed stack's tile.
library-tile-failed-detail = Open to see why and retry
# Under a tile of a stack with several renders, or whose cover is
# starred. $render: the cover by its number, with the star right after it
# ("#4 ★"); $settings: its settings.
library-tile-detail = { $render } · { $settings }
# A batch run's heading line. $n: how many stacks; $time: when it started
# ("2:32 PM").
library-batch-about = Batch · { $n ->
    [one] 1 stack
   *[other] { $n } stacks
} · { $time }
# A batch run's heading line with failures. $n: how many stacks; $time:
# when it started ("2:32 PM"); $failed: how many failed.
library-batch-about-failed = Batch · { $n ->
    [one] 1 stack
   *[other] { $n } stacks
} · { $time } · { $failed } failed
# In a batch run's menu. $n: how many of its stacks failed.
library-batch-delete-failed = { $n ->
    [0] Delete Failed Stacks
    [one] Delete Failed Stack
   *[other] Delete { $n } Failed Stacks
}
# In a batch run's menu. $n: how many stacks it made.
library-batch-delete = { $n ->
    [one] Delete Batch (1 Stack)
   *[other] Delete Batch ({ $n } Stacks)
}

## The Library's menus of a stack's tile (renders/library.rs).

library-menu-open = Open
library-menu-open-to-retry = Open to Retry
library-menu-export = Export…
# In a stack of several. $number: the cover's number.
library-menu-export-number = Export #{ $number }…
# In a stack of several. $number: the cover's number.
library-menu-star-number = Star #{ $number }
# In a stack of several. $number: the cover's number.
library-menu-unstar-number = Unstar #{ $number }
library-menu-rename = Rename Stack…
library-menu-keep-starred = Keep Only Starred
# Keep Only Starred, with how many it deletes. $n: how many unstarred
# renders.
library-menu-keep-starred-delete = { $n ->
    [one] Delete 1 Unstarred Render
   *[other] Delete { $n } Unstarred Renders
}
library-menu-delete-stack = Delete Stack
library-menu-delete-failed = Delete Failed Stack
# With several stacks selected. $n: how many.
library-menu-delete-stacks = { $n ->
    [one] Delete 1 Stack
   *[other] Delete { $n } Stacks
}

## The Library's Rename Stack dialog (renders/library.rs).

library-rename-title = Rename Stack
library-rename-text = Every render of this stack takes the new name. Leave it empty to use the folder's name.
library-rename-placeholder = The folder's name
library-rename-ok = Rename

## The Library's inspector (renders/library.rs): the selected stack and
## its renders.

library-details-none = Select a stack to see its renders.
# The heading with several stacks selected. $n: how many.
library-details-selected = { $n ->
    [one] 1 stack selected
   *[other] { $n } stacks selected
}
# Under the heading. $n: how many renders.
library-details-renders = { $n ->
    [0] No renders
    [one] 1 render
   *[other] { $n } renders
}
# Under the heading. $n: how many renders; $size: what they take on disk
# ("610 MB").
library-details-renders-size = { $n ->
    [0] No renders
    [one] 1 render
   *[other] { $n } renders
} · { $size }
library-details-delete = Delete
library-details-unstacked = Not stacked yet. Its frames and settings are kept. Open the stack to go on.
# A row's label: how many frames.
library-details-frames = Frames
# A row's label: the frames' folder.
library-details-folder = Folder
# A row's label: when a failed stack was tried.
library-details-tried = Tried
# A row's label: the batch run.
library-details-batch = Batch
# A row's label: when the newest render was made.
library-details-newest = Newest
# The Frames row when some were left out. $stacked: how many it stacked;
# $total: how many were listed.
library-details-frames-of = { $stacked } of { $total }
# $error: why the stack didn't finish.
library-details-failed = This stack didn't finish. { $error }
library-details-open = Open
library-details-open-to-retry = Open to Retry
library-details-rename = Rename…
library-details-rename-tip = Rename this stack: every render of it takes the name
# $number: the render shown's number.
library-details-export = Export #{ $number }…
library-details-export-tip = Choose where to export the render shown above
# In the Library's bar while its Export… reads a render, before asking
# where to export it. $number: the render's number.
library-export-reading = Reading #{ $number }…
# A notice: Export… in the Library couldn't read the render.
library-export-read-failed = The render couldn't be read to export it.
# The heading over the stack's renders.
library-details-renders-heading = Renders
# The box about the working version of the stack open in Stacking.
library-working = Working version
# The box about a working version kept for a stack.
library-working-kept = Working version kept
# $edited: what has changed ("Edited since #4", "2 changes since #4").
library-working-edited = { $edited }. Open the stack to go on.
# $number: the render it started from.
library-working-started = Started from #{ $number }. Open the stack to go on.
library-working-never-stacked = Frames never stacked.

## A render's title and label (renders/mod.rs), in compare labels, the
## source menu and the Library's preview. A stack's only render is
## numbered too ("bee #1").

# $stack: the stack's name; $render: the render by its number, with its
# own name and star when it has them ("#4", "#4 ★ · sharper", from the
# engine's render-number messages).
render-title = { $stack } { $render }
# $title: the render's title; $settings: its settings in short
# ("Pyramid · 3 frames").
render-label = { $title } · { $settings }

## The frames panel beside the viewer (sidebar.rs): its header, Add's
## menu, the banners over the list and the line under a selection.

sidebar-frames = Frames
# Beside Frames while a stack runs.
sidebar-frames-locked = Locked while stacking
# Beside Frames when some are excluded. $included: how many are
# included; $total: how many are listed.
sidebar-frames-included = { $included } of { $total }
# The button over the list that adds frames, with a menu.
sidebar-add = Add
sidebar-add-frames = Add Frames…
sidebar-add-folder = Add Folder…
# After the shared line saying how many frames the result left out: the
# button that selects them in the frame list.
sidebar-left-out-show = Show
sidebar-left-out-show-tooltip = Select the frames left out in the Frames list
# The banner when listed files are gone. $n: how many.
sidebar-missing-title = { $n ->
    [one] 1 frame can't be found
   *[other] { $n } frames can't be found
}
# $n: how many listed files are gone.
sidebar-missing-text = { $n ->
    [one] It was moved or renamed. Locate it, or remove it from the list.
   *[other] They were moved or renamed. Locate them, or remove them from the list.
}
# $n: how many listed files are on a drive or share that isn't
# connected.
sidebar-not-connected-title = { $n ->
    [one] 1 frame not connected
   *[other] { $n } frames not connected
}
# $n: how many listed files are on a drive or share that isn't
# connected.
sidebar-not-connected-text = { $n ->
    [one] It's on a drive or share that isn't connected. Connect it, Locate it elsewhere, or remove it from the list.
   *[other] They're on a drive or share that isn't connected. Connect it, Locate them elsewhere, or remove them from the list.
}
sidebar-locate = Locate…
# $n: how many listed files are gone.
sidebar-locate-tooltip = { $n ->
    [one] Choose the folder the frame was moved to
   *[other] Choose the folder the frames were moved to
}
sidebar-remove-missing = Remove
sidebar-remove-missing-tooltip = Take the missing frames out of the list
# Under two or more selected rows. $count: how many are selected.
sidebar-selected = { $count } selected · right-click for actions
# Before the RAW / JPEG switch for shots that have both.
sidebar-pairs = RAW + JPEG pairs
sidebar-pair-raw-tooltip = Stack the RAW of each RAW + JPEG pair
sidebar-pair-jpeg-tooltip = Stack the JPEG of each RAW + JPEG pair, for a faster stack
# The frame order warning, with more folders. $note: what the order
# check found in the first folder; $more: how many more folders.
sidebar-order-more = { $note } (and { $more } more)
# The frame order warning in name order. $note: what the order check
# found.
sidebar-order-wrong = { $note }. Out-of-order frames can stack badly and make a sequence jump. Sort by Capture Time to fix it.
# The frame order warning in name order, with more folders. $note: what
# the order check found in the first folder; $more: how many more
# folders.
sidebar-order-more-wrong = { $note } (and { $more } more). Out-of-order frames can stack badly and make a sequence jump. Sort by Capture Time to fix it.

## The bottom of the inspector (sidebar.rs): what Stack will use, or the
## progress of a sequence or batch.

sidebar-cancel-sequence-tooltip = Cancel the sequence and keep the frames written so far
# $total: the list's frames, all included.
sidebar-stack-uses = { $total ->
    [one] Stack will use 1 frame
   *[other] Stack will use { $total } frames
}
# Some of the list's frames are excluded. $included: how many are
# included, at least 2; $total: how many it lists, at least 3.
sidebar-stack-uses-some = Stack will use { $included } of { $total } frames
sidebar-settings-unlock = Settings unlock when this stack finishes or is canceled.

## The paint panel in the inspector, in Retouch (app/retouch.rs).

sidebar-brush = Brush
sidebar-brush-tooltip = Paint from the source
sidebar-eraser = Eraser
sidebar-eraser-tooltip = Bring back the stacked result
sidebar-paint-tools-note = Brush paints from the source. Eraser brings back the stacked result.
# The brush's sliders.
sidebar-size = Size
sidebar-hardness = Hardness
sidebar-opacity = Opacity
sidebar-flow = Flow
# Over each brush slider: what it sets.
sidebar-size-tooltip = The brush's radius, in pixels of the picture
sidebar-hardness-tooltip = How sharp the brush's edge is: lower fades from the center, to blend a fix in softly
sidebar-opacity-tooltip = The most a stroke changes a pixel toward the source, however often you go over it
sidebar-flow-tooltip = How much each pass adds, up to the opacity: below 100%, a stroke builds up as you go over a spot again
sidebar-colors-match = Only where colors match
sidebar-colors-match-note = Skips pixels where the source differs a lot from the result, so halos and shifted edges don't get painted in.
# The slider of how close the colors must be.
sidebar-within = Within
sidebar-within-tooltip = How far the source's color may differ from the result's and still be painted
# The heading of the key list, and what each key does.
sidebar-keys = Keys
sidebar-key-size = brush size
# { and }, which are Shift-[ and Shift-] on most keyboards.
sidebar-key-hardness = brush hardness
sidebar-key-pick-frame = click to paint from the sharpest frame there
sidebar-key-frame = previous or next frame
sidebar-key-opacity = opacity, 10% to 100%
sidebar-key-peek = see the source
# Space held while dragging over the picture.
sidebar-key-pan = held, drag to pan
sidebar-key-done = Done Retouching
# Under the brush.
sidebar-paint-opening = Opening the render…
sidebar-paint-loading = Loading the source…
# The buttons under why a render couldn't be opened for retouching.
sidebar-paint-try-again = Try Again
sidebar-paint-done = Done
sidebar-paint-nothing = Nothing painted yet
# $strokes: how many strokes; $frames and $renders: how many frames and
# renders they were painted from.
sidebar-paint-strokes-from = { $strokes ->
    [one] 1 stroke
   *[other] { $strokes } strokes
} from { $renders ->
    [0] { $frames ->
        [one] 1 frame
       *[other] { $frames } frames
    }
   *[other] { $frames ->
        [0] { $renders ->
            [one] 1 render
           *[other] { $renders } renders
        }
       *[other] { $frames ->
            [one] 1 frame
           *[other] { $frames } frames
        } and { $renders ->
            [one] 1 render
           *[other] { $renders } renders
        }
    }
}
sidebar-paint-reset = Reset…
sidebar-paint-reset-tooltip = Go back to the stacked result and clear the undo history
# Asked before Reset.
sidebar-reset-title = Reset to the original?
sidebar-reset-text = Every painted pixel goes back to the original and the undo history is cleared. This can't be undone.
sidebar-reset-confirm = Reset

## Remove All Frames' question, for frames never stacked (sidebar.rs).

sidebar-clear-title = Remove all frames?
sidebar-clear-text = These frames were never stacked, so the list can't be brought back. The files stay on disk.
sidebar-clear-confirm = Remove All

## The file list (files.rs): its right-click menu, a row's eye and its
## empty state. Remove All Frames and Sort by Capture Time are also on
## the panel's Add menu and order banner.

files-include = Include
files-exclude = Exclude
files-include-only = Include Only These
files-exclude-after-this = Exclude Frames After This
files-exclude-after-these = Exclude Frames After These
files-remove-from-list = Remove from List
files-remove-frames-from-list = Remove Frames from List
files-include-all = Include All
files-exclude-all = Exclude All
files-sort-by-name = Sort by Name
files-sort-by-capture-time = Sort by Capture Time
files-remove-all-frames = Remove All Frames
# A row's eye button.
files-eye-missing = Can't be found: Locate it or remove it
files-eye-not-connected = On a drive or share that isn't connected: connect it, Locate it or remove it
files-eye-locked = Locked while stacking
files-eye-exclude = Exclude from the stack (X)
files-eye-include = Include in the stack (X)
# On a row whose file is gone.
files-missing = Missing
# On a row whose file is on a drive or share that isn't connected.
files-not-connected = Not Connected
files-empty-title = No frames yet
files-empty-text = Drop frames or a folder here, or click Add.
# How many a list stacks, when it has none.
files-summary-none = No frames
# How many a list stacks when some are excluded. $included: how many
# are included, fewer than all; $total: how many are listed.
files-summary-some = { $included ->
    [0] { $total ->
        [one] 1 frame, excluded
       *[other] { $total } frames, all excluded
    }
   *[other] { $included } of { $total } frames
}

## Adding frames (app/list.rs): the dialogs and the notices.

# The button of the dialog that picks images and folders at once.
list-add-prompt = Add
list-add-frames-title = Add Frames
# The dialog's file type filter.
list-images-filter = Images
list-add-folder-title = Add Folder
# Files dropped on the window or handed over by the system (Open With, a
# second launch) while a stack, batch or sequence runs, which are not added.
list-not-added-running-stack = The frames weren't added, because a stack is running. Add them when it finishes.
list-not-added-running-batch = The frames weren't added, because a batch is running. Add them when it finishes.
list-not-added-running-sequence = The frames weren't added, because a sequence is being written. Add them when it finishes.
list-new-stack-instead = New Stack Instead
# The notice the first time camera RAW is added without the converter.
list-converter-suggestion = { -app-name } reads RAW files on its own. The free Adobe DNG Converter adds Adobe's camera profiles and a second RAW decoder. Restart { -app-name } after installing it. Settings ▸ RAW has this link too.
list-get-it = Get It
# The title of the dialog that finds a moved folder. $folder: its name.
list-locate-title = Locate "{ $folder }"
# After Locate…, every frame of the folder was found in the one picked.
# $n: how many; $folder: the folder picked's name.
list-located-all = { $n ->
    [one] Found the frame in "{ $folder }".
   *[other] Found all { $n } frames in "{ $folder }".
}
# After Locate…, none of the folder's frames were found in the one picked
# as they were (by name, size and contents). $n: how many were looked
# for; $folder: the folder picked's name.
list-located-none = { $n ->
    [one] The frame wasn't found in "{ $folder }" as it was. It may have been moved, renamed or changed.
   *[other] None of the { $n } frames were found in "{ $folder }" as they were. They may have been moved, renamed or changed.
}
# After Locate…, some frames weren't found in the folder picked as they
# were (by name, size and contents). $found: how many were; $of: how many
# were looked for; $folder: the folder picked's name.
list-located-some = { $of ->
    [one] Found { $found } of { $of } frame in "{ $folder }".
   *[other] Found { $found } of { $of } frames in "{ $folder }". The rest may have been moved, renamed or changed.
}

## The version history (app/working.rs, app/navigate.rs): the title bar's words, and
## Revert's confirmation.

# The lead of the error when switching stacks fails.
history-not-switched = The other stack wasn't opened, because this stack's working version couldn't be saved.
# The title bar while a stack runs. $number: the render's number;
# $method: the stacking method; $n: how many frames it stacks.
history-title-stacking = Stacking #{ $number } · { $method } · { $n ->
        [one] 1 frame
       *[other] { $n } frames
    }
# The title bar's second line with no render on screen: the working
# version. $method: the stacking method; $n: how many frames are listed.
history-title-working = { $method } · { $n ->
        [one] 1 frame
       *[other] { $n } frames
    }
# $number: the working version's base render's number.
history-edited-since = Edited since #{ $number }
# The title bar's second line: the render on screen. $render: its number
# and name ("#3", "#3 · sharper"); $method: its method; $frames: what it
# stacked ("44 of 46 frames").
history-title-render = { $render } · { $method } · { $frames }
# The title bar's third line, with a dot, once the working version has
# changed since its base.
history-title-edited = Edited
# The title bar's third line before the stack's first render.
history-title-not-stacked = Not stacked yet
# The confirmation's button that reverts, losing the unstacked changes
# (its title and text are the engine's).
revert-alert-revert = Revert

## The settings panel in the inspector (params.rs).

settings-method = Method
settings-alignment = Alignment
# Beside Alignment while its row is closed: what is set. $aligner: the
# aligner's name ("ECC (affine)").
settings-alignment-cropped = · { $aligner }, cropped
# $aligner: the aligner's name ("ECC (affine)").
settings-alignment-uncropped = · { $aligner }, uncropped
# Under the Method picker: what the method chosen is for and its one
# cost, in a line. Its whole description is its segment's tooltip.
settings-method-pyramid-note = The all-around choice; front edges can glow
settings-method-depth-note = Clean backgrounds and edges; softer hairs
settings-method-wavelet-note = For fine crossing detail; grainier backgrounds
# The sliders and the words at their ends.
settings-smoothness = Smoothness
settings-smoothing = Smoothing
settings-denoise = Denoise
settings-radius = Radius
settings-blend-range = Blend Range
settings-crisp = Crisp, more noise
settings-calm = Calm, softer
settings-all-detail = All detail
settings-less-noise = Less noise
settings-narrow-outlines = Narrow outlines
settings-even-backgrounds = Even backgrounds
settings-thin-detail = Thin detail
settings-steadier = Steadier
settings-one-frame = One frame per spot
settings-mixed = Mixed, softer
settings-advanced = Advanced
# Beside Advanced while it is closed.
settings-advanced-hidden = · Blend Range
# On a slider's name. $value: the default, as the slider shows it.
settings-slider-reset-tooltip = Double-click to reset to { $value }
# At the end of the method's sliders: puts every one back to its default.
settings-reset-defaults = Reset to Defaults
# $method: the method's name (Pyramid, Depth, Wavelet).
settings-reset-defaults-tooltip = Set { $method }'s sliders back to their defaults
settings-crop = Crop to the area every frame covers
# The RAW decoder picker's label.
settings-decoder = RAW decoder
# Beside the change line: the working version takes the render shown's
# frames and settings.
settings-revert = Revert
# $number: the render shown.
settings-revert-tooltip = Make the frames and settings those of #{ $number }

## The view bar over the photo (app/view_bar.rs, app/scrubber.rs): the
## home control (the render shown, and its stack's other renders), the
## focus rail through the stack's frames, Compare and its layouts, Swap,
## Contribution, the depth map, zoom and focus mode. In Retouch, its
## layouts; Paint From takes the home control's place and the rail picks
## the frame painted from.

# The home control before the result is saved as a render.
view-bar-home-result = Result
view-bar-home-tooltip = Show the result alone
view-bar-home-menu = The renders of this stack
# Retouch's Paint From pop-up, over its button.
view-bar-paint-from = Paint From
# Paint From's title while a frame is painted from (the rail says which),
# and its menu's first item.
view-bar-paint-from-frames = Frames
# Paint From's title while the original is painted back.
view-bar-paint-from-original = Original
# Paint From's menu's last item, beside the eraser's key.
view-bar-original = Original (paint it back)
# Over a tick of the focus rail. $number: the frame's place in the stack;
# $name: its file name; $share: its share of the result, finished ("14%").
view-bar-rail-frame-share = Frame { $number } · { $name } · { $share }
# The same for a stack with no record of where each frame was used.
# $number: the frame's place in the stack; $name: its file name.
view-bar-rail-frame = Frame { $number } · { $name }
# The same for a frame the stack left out. $number: the frame's place
# in the stack; $name: its file name.
view-bar-rail-frame-left-out = Frame { $number } · { $name } · left out
# A share under one percent, before the percent sign ("<1%").
view-bar-rail-under-one = <1
# Beside the rail. $number: the frame on screen's place ("18", or "–" for
# none); $count: how many frames the stack has ("46").
view-bar-rail-position = { $number }/{ $count }
# The tooltip of that place beside the rail. $key: the modifier ("⌥" on
# macOS, "Alt" elsewhere).
view-bar-rail-position-tooltip = The frame on screen, of the stack's frames. { $key }-drag along the rail to move one frame at a time.
# The tooltip of that place while a stack runs, where it counts the frames
# the stack has taken in so far ("9/66"). Takes no arguments.
view-bar-rail-running-tooltip = The frames stacked so far, of the stack's frames. Each takes its color on the depth map as the stack reaches it. Locked while stacking.
# The button that turns comparing on and off.
view-bar-compare = Compare
view-bar-compare-toggle = Compare the result with a frame or another render, in the layout used last
# The same in Retouch, where it lays the source out beside the canvas.
view-bar-compare-toggle-retouch = Compare the painted result with its source, in the layout used last
# The menu beside Compare, and its items.
view-bar-layouts = Choose how to compare
view-bar-layout-result = Result
# The first item in Retouch: the painted result alone.
view-bar-layout-painted = Painted
view-bar-layout-split = Split
view-bar-layout-side-by-side = Side by Side
view-bar-layout-top-and-bottom = Top and Bottom
view-bar-layout-flicker = Flicker
view-bar-swap = Show the compared render, and compare with this one
view-bar-contribution = Contribution: what the result took from this frame, shown as it is, the rest dimmed
# Contribution's tooltip while it can't be shown.
view-bar-contribution-unavailable = Contribution: show a frame this stack used, of a render that records where its detail came from
# Contribution's tooltip in Retouch, where it is off.
view-bar-contribution-retouch = Contribution: off in Retouch, which shows the source painted from as it is
view-bar-depth-map = Depth Map: which frame each part of the result came from
# The same while the map is shown: the rail's ticks are in its colors.
view-bar-depth-map-key = Depth Map: the frames on the rail are in its colors; press one, or click the map, to show what that frame supplied
# The same in Retouch, where the frame picked is the one painted from.
view-bar-depth-map-key-retouch = Depth Map: the frames on the rail are in its colors; the map shows what the frame painted from supplied, and a press on the rail or a click on the map picks another
# The same for a render saved without a record of which frame each part
# came from.
view-bar-depth-map-unavailable = Depth Map: this render doesn't record which frame each part of the result came from
view-bar-zoom-field = Zoom: type a percentage
view-bar-zoom-presets = Choose a zoom
view-bar-zoom-to-fit = Zoom to Fit
view-bar-actual-size = Actual Size
view-bar-zoom-out = Zoom Out
view-bar-zoom-in = Zoom In
view-bar-focus-mode = Focus Mode: only the picture and the bars

## The title bar (app/toolbar.rs), in Stacking and in the Library.

toolbar-toggle-sidebar = Show or Hide Frames
# The button back to the Library.
toolbar-library = Library
toolbar-library-tooltip = Go back to the Library, where every stack is
# The title of a stack with nothing in it.
toolbar-new-stack-title = New stack
toolbar-retouch = Retouch
toolbar-retouch-tooltip = Paint over the result from a frame or another render
toolbar-export = Export
toolbar-export-tooltip = Export the result: format, name and folder
toolbar-stack = Stack
toolbar-stack-tooltip = Make a render from the working version
toolbar-toggle-inspector = Show or Hide Inspector
# The Library's title.
toolbar-library-title = Library
toolbar-new-batch = New Batch…
toolbar-new-batch-tooltip = Stack many folders at once, or sort them into stacks by capture time
toolbar-new-stack = New Stack
toolbar-new-stack-tooltip = Start a new stack, then add its frames
toolbar-open = Open
toolbar-open-tooltip = Open the selection in Stacking

## Comparing with a render (app/compare.rs): the panes' labels.

# What the result is called when it has no label.
compare-result = Result
# The reference pane. $name: the render's label.
compare-a = A · { $name }
# The pane open. $name: the render's label.
compare-b = B · { $name }

## The dialogs' shared pieces (form.rs).

# The checkbox that shows an export's file once it is written.
form-reveal-macos = Show in Finder when done
form-reveal-windows = Show in Explorer when done
form-reveal-other = Show in File Manager when done

## The New Batch… dialog (batch_dialog.rs): the folders, how to split them,
## pairs, the frame order, the RAW decoder, After stacking and the summary.

# The dialog's title: the command that opens it.
batch-dialog-title = New Batch
batch-folders-heading = Folders
batch-add-folders = Add Folders…
batch-add-subfolders = Add Subfolders of…
batch-add-subfolders-tooltip = Add each folder inside the one you choose, without the frames in that folder itself
batch-use-list = Add the Frames List
batch-use-list-tooltip = Add the frames included in the Frames list, as one more source
# The name of the Frames list in the dialog's list of sources, once it is
# added.
batch-frames-list = Frames list
batch-include-deeper = Include deeper folders
# A source's button that takes it out of the list.
batch-remove = Remove
# The list of sources, while it is empty.
batch-no-folders = No folders yet
batch-split-heading = How to split
batch-split-per-folder = One stack per folder
batch-split-auto-sort = Auto sort: split each folder into stacks by capture time
batch-pairs = RAW + JPEG pairs
batch-pairs-raw = Use RAW
batch-pairs-jpeg = Use JPEG
batch-order = Frame order
batch-order-name = Name
batch-order-capture-time = Capture time
batch-raw-decoder = RAW decoder
batch-after-heading = After stacking
batch-start = Start Batch

# The export's JPEG quality, in a dropdown. $quality: the quality (1–100).
batch-export-quality = Quality { $quality }
batch-export-each = Export each render
batch-export-sixteen-bit = 16 bits per channel
batch-export-note = Each file is named after its stack and written as soon as its render is made. DNG is for RAW stacks; other stacks are written as TIFF.
# The button that picks the folder an export or the sequence goes to.
batch-choose-folder = Choose Folder…
# Beside Choose Folder…, until a folder is picked.
batch-no-folder-chosen = No folder chosen
batch-turntable = Turntable sequence
# Under Turntable sequence, when Auto sort is chosen.
batch-turntable-needs-per-folder = Needs one stack per folder
batch-turntable-needs-pyramid = Grow across stacks and Both need Pyramid. With this method the sequence is Full: one finished render per angle.
batch-direction-back-to-front = Back to front
batch-direction-front-to-back = Front to back
# The grow sequence's length, beside its dropdown.
batch-length = Length
# The length dropdown's first choice.
batch-length-stack-depth = Same as stack depth
# Beside the length, with the stack depth chosen and known. $n: the stacks'
# usual frame count.
batch-length-per-step = { $n ->
    [one] 1 frame, one per focus step
   *[other] { $n } frames, one per focus step
}
# Beside the length, with the stack depth chosen and the folders not yet
# read.
batch-length-per-step-unknown = One frame per focus step
# Beside the length, with a number of frames chosen.
batch-length-spread = The whole turn and the growing focus are spread over this many frames
batch-turntable-note = Folders stack in name order, aligned and cropped to the area every frame covers. A stack that looks like it began with a stray frame is stacked again without that frame.

# The summary, while the folders inside a parent are listed.
batch-summary-listing = Looking in the folders…
# The summary, with nothing listed.
batch-summary-empty = Add the folders to stack
# The summary, while the plan is worked out.
batch-summary-working = Working out the stacks…
# $address: the support email address.
batch-summary-failed = The stacks couldn't be worked out. { -app-name } ran into an unexpected problem. Help ▸ Save Diagnostics… gathers the details to email to { $address }.
# The batch's closing notice when an export or its sequence failed.
# $short: its counts, exports and sequence, joined by " · "; $why: why,
# in full sentences.
batch-closing-reasons = { $short }. { $why }
# The batch's closing notice's button: the Library with only the failed
# stacks shown.
batch-show-failed = Show Failed
# The summary of a turntable run, with the folders it goes through.
# $summary: the plan's summary ("3 folders → 3 renders"); $first, $last:
# the first and last folder's names.
batch-summary-range = { $summary } · { $first } to { $last }
# The summary with a grow turntable. $summary: the summary so far; $n: the
# sequence's frames.
batch-summary-turntable-grow = { $summary } · turntable Grow across stacks, { $n ->
    [one] 1 frame
   *[other] { $n } frames
}
# The summary with a full turntable. $summary: the summary so far; $n: the
# sequence's frames, one per stack.
batch-summary-turntable-full = { $summary } · turntable Full, { $n ->
    [one] 1 frame
   *[other] { $n } frames
}
# The summary with both turntables. $summary: the summary so far; $grow:
# the grow sequence's frames; $full: the full sequence's frames.
batch-summary-turntable-both = { $summary } · turntable Grow across stacks, { $grow ->
    [one] 1 frame
   *[other] { $grow } frames
}, and Full, { $full ->
    [one] 1 frame
   *[other] { $full } frames
}
# The summary with a note about the frame order. $summary: the summary so
# far; $note: the note, a stack's name and a lowercase clause ("s036: the
# frames were shot…"), joined as the summary's other parts are.
batch-summary-note = { $summary } · { $note }
# The summary with the first of several notes. $summary: the summary so
# far; $note: the first note; $more: how many more there are.
batch-summary-notes = { $summary } · { $note } (and { $more } more)
# Under the summary. $settings: the batch-settings line.
batch-settings-pairs-raw = { $settings } · RAW + JPEG pairs: Use RAW
# Under the summary. $settings: the batch-settings line.
batch-settings-pairs-jpeg = { $settings } · RAW + JPEG pairs: Use JPEG
# The settings the batch runs with. $summary: the method and its
# settings.
batch-settings = Uses the inspector's settings: { $summary }
# $summary: the method and its settings.
batch-settings-grid = Uses the inspector's settings: { $summary } · Grid search
# $summary: the method and its settings.
batch-settings-uncropped = Uses the inspector's settings: { $summary } · uncropped
# $summary: the method and its settings.
batch-settings-grid-uncropped = Uses the inspector's settings: { $summary } · Grid search · uncropped

## A batch run's notices (app/batch_run.rs, app/batch_after.rs). The
## closing summary is the shared layer's.

batch-busy-stack = A batch can't start while a stack runs. Wait for it to finish.
batch-busy-sequence = A batch can't start while a sequence is written. Wait for it to finish.
batch-busy-batch = A batch is already running. Wait for it to finish, or cancel it.
batch-nothing-to-stack = The batch has nothing to stack.
# $why: why not, in sentences.
batch-cant-start = The batch can't start. { $why }
# Why a batch's stack failed, after its name.
batch-stack-didnt-start = It couldn't start.
batch-stack-no-render = It made no render.
batch-stack-no-thumbnail = Its thumbnail couldn't be made.
# $fault: the fault sentence and where its details are.
batch-turntable-fault = The turntable sequence wasn't written. { $fault }

## The Export sheet (export_dialog.rs).

export-format = Format
export-format-note-tiff = TIFF keeps 16 bits per channel, lossless.
export-format-note-jpeg = JPEG is 8 bits per channel, at the quality set in Settings ▸ Export: for sharing.
export-format-note-png = PNG keeps 16 bits per channel too, lossless, in larger files than TIFF.
export-format-note-dng = DNG keeps the linear camera data, for your RAW editor to develop.
# Why DNG is dimmed, under the format's note.
export-no-dng = DNG is only for stacks of RAW or DNG frames.
export-bit-depth = Bit depth
export-sixteen-bits = 16 bits per channel
export-eight-bits = 8 bits per channel
# Over each bit depth: what it keeps.
export-sixteen-bits-tooltip = Keeps the full range of tones the result has, for further editing
export-eight-bits-tooltip = Smaller files that every app opens, for sharing
export-color = Color
export-color-dng = Linear camera color, with the frames' camera profile
export-color-developed = sRGB, developed through the frames' camera profile
export-color-srgb = sRGB, like the frames
# $profile: the frames' color space, the name its profile gives itself
# ("Display P3").
export-color-profile = { $profile }, like the frames
# The frames' color space, when its profile gives itself no name.
export-color-unnamed-profile = An unnamed color profile, like the frames
export-name = Name
export-name-note = Starts from the file name set in Settings ▸ Export.
export-save-to = Export to
# Where an export goes: in the sheet, and in Settings ▸ Export.
export-place-frames = Next to the frames
export-place-last = Last folder used
# $keys: Export Again's shortcut.
export-again-note = Export Again ({ $keys }) reuses these choices.
# The sheet's button.
export-button = Export

## Export notices and the sheet's header (app/export.rs).

# The sheet's title, for a result not saved as a render.
export-title = Export
# The sheet's title. $number: the render's number.
export-title-numbered = Export #{ $number }
# The name a stack with none goes by, in the sheet and the file name.
export-unnamed-stack = stack
# Under the sheet's title. $stack: the stack's name; $settings: its method
# and settings ("Pyramid · S4").
export-subtitle = { $stack } · { $settings }
# Under the sheet's title, for a retouched render. $stack: the stack's
# name; $settings: its method and settings ("Pyramid · S4").
export-subtitle-retouched = { $stack } · { $settings } · retouched
# $name: the file name.
export-done = Exported { $name }
# In the Export button's place while an export is written. $name: the
# file name.
export-exporting = Exporting { $name }…

## The Export Grow Sequence… dialog (grow_dialog.rs), and the frame
## options it shares with Batch's turntable sequence.

# The dialog's title.
grow-dialog-title = Export Grow Sequence
# $name: the render's name.
grow-intro = Each frame of the sequence stacks one more of { $name }'s frames than the frame before it, so the picture comes into focus as it plays. The last frame is the render.
grow-direction-heading = Direction
grow-direction-back-to-front = Back to front: focus arrives from behind
grow-direction-front-to-back = Front to back: nearest first
grow-frames-heading = Frames
grow-no-retouching = Retouching isn't included, because the sequence is stacked again from the frames.
# $n: the frames; $size: the frame size ("8138 × 5449"); $bytes: the size on
# disk ("5.4 GB").
grow-summary = { $n ->
    [one] 1 frame
   *[other] { $n } frames
} · { $size } · about { $bytes }
grow-written = Written as numbered frames in a new folder, for a video editor such as DaVinci Resolve to import as one clip.
# The dialog's button, which goes on to ask where.
grow-export = Export…
grow-full-size = Full size
# A frame size, in a dropdown. $pixels: the long edge.
grow-long-edge = { $pixels } px long edge
grow-sixteen-bit = 16 bits per channel (TIFF and PNG)

## Sequences (sequences.rs, app/sequence.rs): the progress line and the
## notices.

sequence-grow-starting = Grow sequence · starting
sequence-turntable-starting = Turntable sequence · starting
# $made: what was made, the shared layer's lines joined by " · ".
sequence-exported = Exported sequence: { $made }
sequence-canceled = Sequence canceled. The frames already written are kept.
sequence-didnt-finish = The sequence didn't finish.
sequence-pyramid-only = Only a Pyramid render can make a grow sequence.
# A missing frame's name, in the list of them. $name: the file name.
sequence-missing-name = "{ $name }"
# The last two names of the missing frames. $names: the names before the
# last, as a list (the shared layer's list-comma); $last: the last name.
sequence-missing-names = { $names } and { $last }
# The missing frames, beyond the first three. $names: the first three
# names, as a list (the shared layer's list-comma); $more: how many more.
sequence-missing-names-more = { $names } and { $more } more
# $n: how many frames are missing; $listed: their names.
sequence-missing = { $n ->
    [one] The sequence needs this render's frames, and 1 of them can't be found: { $listed }.
   *[other] The sequence needs this render's frames, and { $n } of them can't be found: { $listed }.
}
# The missing frames notice's button.
sequence-locate = Locate…

## A stack run (app/run.rs): its progress card and pill, and a failed
## stack's card.

# The progress card's title. $number: the render's number.
run-stacking-numbered = Stacking #{ $number }
run-stacking = Stacking
# The progress card's first step while the first frame is read.
run-step-reading = Reading the frames
# A step on the progress card. $loaded: frames aligned; $total: the
# run's frames.
run-step-aligning = Aligning { $loaded } of { $total }
# The first step when the frames turned out to be in reverse order and
# aligning starts over with them turned round.
run-step-restarting = The frames were in reverse order. Aligning again
run-step-aligned = Aligned
run-step-blending = Blending
# The progress card once Cancel is asked, until the stack stops.
run-step-canceling = Canceling…
# The progress pill. $percent: how far ("68").
run-pill-reading = Reading · { $percent }%
# The progress pill. $percent: how far ("68").
run-pill-aligning = Aligning · { $percent }%
# The progress pill, aligning again with the frames turned round.
# $percent: how far ("12").
run-pill-restarting = Restarting · { $percent }%
# The progress pill. $percent: how far ("68").
run-pill-blending = Blending · { $percent }%
# The progress pill as the result is presented. $percent: how far ("100").
run-pill-finishing = Finishing · { $percent }%
# The progress pill once Cancel is asked, until the stack stops.
run-pill-canceling = Canceling…
# A batch's progress pill: $status is the batch's line ("Batch 2 of 5 ·
# Bee"), $step the step its stack is on ("Aligning").
run-pill-batch = { $status } · { $step }
# The step a batch's stack is on, after the batch's line.
run-phase-reading = Reading
run-phase-aligning = Aligning
run-phase-restarting = Restarting
run-phase-blending = Blending
run-phase-finishing = Finishing
run-cancel-stack-tooltip = Cancel the stack
run-cancel-batch-tooltip = Cancel the batch
# Why a button is dimmed while a stack runs.
run-busy-stack = Not while a stack runs: wait for it to finish, or cancel it
# Why a button is dimmed while a batch runs.
run-busy-batch = Not while a batch runs: wait for it to finish, or cancel it
# Why a button is dimmed while a grow sequence is written.
run-busy-sequence = Not while a sequence is written: wait for it to finish, or cancel it
# $seconds: whole seconds.
run-seconds-left = About { $seconds } s left
# $minutes: whole minutes.
run-minutes-left = About { $minutes } min left
# A failed stack's message, before why. $number: the render's number.
run-failure-lead-numbered = Stack #{ $number } didn't finish.
run-failure-lead = The stack didn't finish.
# The failure card's title. $number: the render's number.
run-failure-title-numbered = Stack #{ $number } didn't finish
run-failure-title = The stack didn't finish
run-failure-dismiss = Dismiss
run-try-again = Try Again
run-copy-details = Copy Details

## A run's end while the window is not in front (app/away.rs): the
## system notification's title, over the in-app notice's text. A failed
## stack's is run-failure-title.

away-stacked = Stack finished
away-batch = Batch finished
away-sequence = Sequence written
away-sequence-failed = The sequence didn't finish

## Errors as the desktop shows them (errors.rs): a lead, then the shared
## layer's reason and advice; for a fault, where its details are.

# $lead: what didn't happen, a sentence; $why: why, in sentences.
error-with-lead = { $lead } { $why }
# A fault, in a message with Copy Details. $reason: the fault sentence.
error-fault-copy = { $reason } Click Copy Details, then choose Help ▸ Report a Problem… and paste them into the report.
# A fault, in a message with no Copy Details: its notice offers Save
# Diagnostics… and Show Log Folder, which are also in the Help menu.
# $reason: the fault sentence; $address: the support email address.
error-fault-diagnostics = { $reason } Help ▸ Save Diagnostics… gathers the details to email to { $address }.
# After a file or folder the system wouldn't let Deptheos open, on macOS:
# Files & Folders is the system's own name for the setting.
error-not-allowed-advice-macos = Allow { -app-name } to use that folder in System Settings ▸ Privacy & Security ▸ Files & Folders, then try again.
# The same, on Windows and Linux.
error-not-allowed-advice = Check that you can open the file, then try again.
# A fault's notice's buttons.
error-save-diagnostics = Save Diagnostics…
error-show-log-folder = Show Log Folder

## File dialogs' titles (batch_dialog.rs, export_dialog.rs,
## app/sequence.rs): the name of the command that opens one, as the menu
## writes it without its ellipsis, else sentence case.

file-dialog-add-folders = Add Folders
file-dialog-add-subfolders = Add Subfolders of
file-dialog-batch-renders = Choose where the renders go
file-dialog-batch-sequence = Choose where the sequence goes
file-dialog-export-to = Export to
file-dialog-grow-sequence = Choose where the sequence goes

## The Settings window's bar of panes (prefs_window.rs): each pane's name,
## under its icon and as the window's title while it is shown. (The RAW
## pane is named by the format alone, in the code.)

prefs-pane-general = General
prefs-pane-export = Export
prefs-pane-library = Library
prefs-pane-alignment = Alignment
prefs-pane-advanced = Advanced

## Buttons and labels the Settings window's panes share.

# Puts a path setting back to its default; also shown in place of a
# folder that is the default.
prefs-default = Default

## Settings ▸ General: Appearance and Updates.

# The group's title.
prefs-appearance = Appearance
prefs-theme = Theme
prefs-theme-description = Light or dark, or as the system is set, changing when it does.
# The Appearance choices: follow the system, or always light or dark.
prefs-appearance-system = System
prefs-appearance-light = Light
prefs-appearance-dark = Dark

prefs-updates = Updates
prefs-check-for-updates = Check for updates
prefs-check-for-updates-description = At startup, ask GitHub whether a newer version is out and offer its download page. Only the app's version is sent.

## Settings ▸ Export. Its 16 bits, TIFF compression, JPEG quality and
## file name are the shared layer's (pref-*, deptheos_app.ftl).

prefs-export-defaults = Defaults for new exports
prefs-format = Format
prefs-format-description = The format Export starts with for a stack of JPEG, PNG or TIFF frames; the sheet remembers the last one used. DNG is for RAW and DNG stacks only.
prefs-format-raw = Format for RAW stacks
prefs-format-raw-description = The format Export starts with for a RAW or DNG stack; remembered the same way. DNG keeps the linear camera data for your RAW editor to develop.
prefs-folder-and-name = Folder and file name
prefs-export-to = Export to
prefs-export-to-description = Where the Export sheet starts.
prefs-reveal-export-description = Once a render is exported, its file is shown.

## Settings ▸ Library.

prefs-renders = Renders
prefs-library-folder = Library folder
prefs-library-folder-description = Where every render is kept. Takes effect the next time { -app-name } starts.
prefs-delete-library = Delete Library
prefs-delete-library-description = Deletes every stack, render and failed stack, with their retouching and the working version, leaving the Library empty. Your frames and exported files are left alone.
# The button that asks before deleting the whole Library.
prefs-delete-library-button = Delete Library…
# Why Delete Library can't be used now, shown under it and as the button's
# tooltip: there is nothing in the Library.
prefs-delete-library-empty = The Library is empty.
# The same: the Library folder couldn't be opened at launch.
prefs-delete-library-unopened = The Library folder couldn't be opened, so there is nothing to delete.
# The same: the Library folder chosen above isn't the one open, which
# changes the next time the app starts. $folder: the path of the one open.
prefs-delete-library-moved = Available once { -app-name } restarts with the folder chosen above. The Library open now is in "{ $folder }".
# The same: the Library is still being read at launch.
prefs-delete-library-listing = The Library is still being read.
# The same: a stack, batch or grow sequence is running.
prefs-delete-library-running = Available once the stack, batch or sequence running finishes.
# The same: a result or a render is still being written.
prefs-delete-library-writing = Available once the files being written are saved.
# The same: a delete is running (this one or another).
prefs-delete-library-deleting = Available once the delete running finishes.
# The same when it isn't known how far it got.
library-delete-unsure = The Library may not all have been deleted.
# Delete Library was confirmed just as something started.
library-delete-not-now = The Library wasn't deleted, because something is running or still being written. Delete it again when that finishes.

## Settings ▸ RAW: Decoding, Camera profile folders and the Adobe DNG
## Converter group (its title is the product's name, in the code).

prefs-decoding = Decoding
prefs-default-decoder = Default RAW decoder
prefs-default-decoder-description = What the inspector's RAW decoder starts with, and what batches use. Automatic is Adobe DNG Converter when it is installed, else LibRaw. The two crop and demosaic differently, so their frames of the same shot don't line up.
# The Default RAW decoder's choice that picks one of the two.
prefs-decoder-automatic = Automatic
prefs-camera-profile = Camera profile
# The camera profile choices: Adobe's two profiles, or none.
prefs-profile-adobe-standard = Adobe Standard
prefs-profile-camera-standard = Camera Standard
prefs-profile-none = None (the colors in the file)
prefs-camera-profile-description = The installed Adobe profile that colors LibRaw-decoded RAW files. DNGs and Adobe-converted frames carry their own. Recorded with each render.
prefs-profile-folders = Camera profile folders
prefs-profile-folders-description = Searched for .dcp files before Adobe's folders. A profile is used when its file name starts with the camera's make and model, for example "Canon EOS R5 Adobe Standard.dcp".
# Under the converter's and the profile folders' settings while a stack,
# batch or sequence runs.
prefs-applies-when-run-ends = Stacking is under way: a change here applies once it ends.
# In place of the list of profile folders.
prefs-no-profile-folders = No folders added.
# Beside a profile folder.
prefs-remove-profile-folder = Remove
prefs-add-profile-folder = Add Folder…
prefs-adobe-profile-folders = Adobe profile folders found
# In place of the list of Adobe's profile folders.
prefs-no-adobe-profile-folders = None found. Adobe's free DNG Converter installs these profiles.
prefs-converter = Converter
prefs-converter-description = Used by the Adobe DNG Converter RAW decoder. Choose the application if it isn't in its default location.
# Under the converter's path: it can't be used on this system.
prefs-converter-unavailable = Not available on this platform
# Under the converter's path: it is there.
prefs-converter-found = Found
# Under the converter's path: it isn't there.
prefs-converter-not-found = Not found. It is free from Adobe; restart { -app-name } after installing it.
# Opens Adobe's download page for the converter.
prefs-converter-get = Get It
prefs-converter-jobs = Parallel conversions
prefs-converter-jobs-description = How many RAW files are converted at once. More is faster until the CPU or disk is saturated; each conversion also uses some memory.
prefs-converter-uncompressed = Uncompressed conversions
prefs-converter-uncompressed-description = Write the converted DNGs without compression. A stack of them runs about a quarter faster, but each takes twice the disk: about 270 MB for a 45 MP frame instead of 130 MB. The conversions are kept until the stack has read them for the last time, and Depth reads every frame twice, so a 200-frame Depth stack needs about 54 GB free instead of 26 GB.

## Settings ▸ Alignment. The grid search limits and exposure compensation
## are the shared layer's (pref-*, deptheos_app.ftl).

## Settings ▸ Advanced: Graphics, Memory and Diagnostics.

prefs-graphics = Graphics
prefs-gpu = GPU
prefs-gpu-description = The GPU that stacks, paints and prepares images. Software rendering runs on the CPU, as on a computer without a GPU: many times slower. Takes effect the next time { -app-name } starts.
# The GPU choices.
prefs-gpu-automatic = Automatic (the fastest GPU)
prefs-gpu-software = Software rendering (on the CPU, slow)
# A GPU found, by its kind. $name: the GPU's name.
prefs-gpu-dedicated = { $name } (dedicated)
# $name: the GPU's name.
prefs-gpu-integrated = { $name } (integrated)
# The CPU renderer, found among the GPUs. $name: its name.
prefs-gpu-cpu = { $name } (software, on the CPU)
# A chosen GPU this computer doesn't have. $name: the GPU's name.
prefs-gpu-not-found = { $name } (not found)
# Under the GPU choice. $device: the GPU the engine runs on.
prefs-gpu-in-use = In use: { $device }
prefs-gpu-none-started = No GPU could be started.
# Under the GPU choice, when it differs from the GPU in use.
prefs-gpu-restart = Restart { -app-name } to use it.
# DEPTHEOS_SOFTWARE_GPU is an environment variable's name.
prefs-gpu-forced = DEPTHEOS_SOFTWARE_GPU is set, which overrides this.
# $reason: why the chosen GPU couldn't start, one or more sentences.
prefs-gpu-failed = This choice couldn't start, so { -app-name } is using the fastest GPU it found. { $reason }
# $name: the chosen GPU's name.
prefs-gpu-missing = { $name } wasn't found, so { -app-name } is using the fastest GPU.
prefs-memory = Memory
prefs-read-ahead = Frame read-ahead (MB)
prefs-read-ahead-description = Decoded JPEG, PNG and TIFF frames held in memory ahead of stacking. Lower it on a computer with little memory; above the default rarely helps.
prefs-diagnostics = Diagnostics
prefs-verbose-logging = Verbose logging
prefs-verbose-logging-description = Until { -app-name } quits. Adds detail to the log for bug reports (Help ▸ Save Diagnostics…).

## The update check's notices (app/update_check.rs, updates.rs).

update-available-title = Update available
# $version: the newer release's version; $current: this build's.
update-available = { -app-name } { $version } is available. You have { $current }.
# Opens the newer release's page.
update-download = Download
# $version: this build's version.
update-up-to-date = { -app-name } { $version } is the latest version.
update-failed-unreachable = { -app-name } couldn't check for updates. The release page couldn't be reached. Check the internet connection, then try again.
# The release host answered with an error (its rate limit, an outage).
update-failed-refused = { -app-name } couldn't check for updates. The release page answered with an error. Try again later.
update-failed-unreadable = { -app-name } couldn't check for updates. The reply about the latest release couldn't be read.
# $tag: the latest release's tag, as GitHub gives it.
update-failed-not-a-version = { -app-name } couldn't check for updates. The latest release is tagged "{ $tag }", which isn't a version number { -app-name } recognizes.

## The About window (app/update_check.rs).

about-title = About { -app-name }
# The app's name, under its icon.
about-name = { -app-name }
# $version: this build's version ("0.3.0"). $build: the build, the short
# commit and profile ("a7e698fe release").
about-version = Version { $version } ({ $build })
about-copy-version = Copy Version
about-copy-version-tooltip = Copy the version and build, with the system's and the GPU's names, for a bug report
about-website = Website
about-website-tooltip = Open { -app-name }'s website
about-tagline = Focus stacking for macOS, Windows and Linux.
about-license-text = Free software under the GNU General Public License version 3, with an additional permission for the Adobe DNG SDK and XMP Toolkit. It includes software by others under their own licenses, listed in the third-party notices.
about-license = License
about-notices = Third-Party Notices

## The empty window's welcome (app/welcome.rs).

# The drop target's heading in the Library.
welcome-drop-library = Drop a focus bracket here
# The drop target's heading in an empty stack.
welcome-drop-stack = Drop this stack's frames here
welcome-drop-formats = A folder, or the frames themselves: RAW, DNG, JPEG, TIFF or PNG
welcome-add-frames = Add Frames…
welcome-add-folder = Add Folder…
# Before the link that tries a sample stack (welcome-sample).
welcome-no-stack = No frames of your own yet?
# The link that tries a sample stack. With two carried it opens a menu of
# them, each with its name (Hornet, Circuit Board) and a line under it
# (welcome-sample-handheld, welcome-sample-tripod); with one it adds that
# one to a new stack, which waits for Stack; in a build without one it
# opens the website's sample stacks.
welcome-sample = Try a sample stack
# In the welcome's menu of sample stacks, under the hornet's name: its
# frames, shot handheld. $count: how many frames (66).
welcome-sample-handheld = { $count ->
    [one] 1 frame, handheld
   *[other] { $count } frames, handheld
}
# In the welcome's menu of sample stacks, under the circuit board's name:
# its frames, shot on a tripod. $count: how many frames (200).
welcome-sample-tripod = { $count ->
    [one] 1 frame, on a tripod
   *[other] { $count } frames, on a tripod
}
# In place of welcome-sample while a sample stack's frames are copied into
# the Library folder.
welcome-sample-copying = Copying the sample stack…
# The three steps under the drop target: a title and a line each.
welcome-step-add = Add frames
welcome-step-add-text = The photos of one subject, each focused a little nearer or farther than the one before.
welcome-step-stack = Stack
# $keys: the keys that stack, written for the platform ("⌘⏎").
welcome-step-stack-text = The inspector's settings suit most stacks as they are. Click Stack ({ $keys }).
welcome-step-export = Retouch and export
# $keys: the keys that export, written for the platform ("⌘E").
welcome-step-export-text = Paint fixes from any frame, then Export ({ $keys }).
welcome-batch = Many folders to stack at once?
# The batch link on the Library, where the batch runs.
welcome-batch-start-library = Start a batch
# The batch link in an empty stack: the batch runs in the Library.
welcome-batch-start-stack = Start a batch in the Library

## Trying a sample stack (app/sample.rs): the lead of the notice when its
## frames can't be copied into the Library folder; the shared layer's
## reason follows.

sample-copy-failed = The sample stack couldn't be copied into the Library folder.

## Quitting while something runs or is being written (app/quit.rs).

quit-batch-running = A batch is running. Quit anyway?
quit-sequence-running = A grow sequence is being written. Quit anyway?
quit-stack-running = A stack is running. Quit anyway?
quit-description = Quitting cancels it. Every render already made is kept, with any files exported and sequence frames written so far.
# The question's button.
quit-button = Quit
quit-waiting = Saving files. { -app-name } quits once they are written.
# Shown while the retouching is saved as the app quits.
quit-saving-retouching = Saving your retouching. { -app-name } quits once it is saved.
# Asked when the retouching couldn't be written to disk as the app quits.
quit-paint-unsaved = Your latest retouching couldn't be saved. Quit anyway?
quit-paint-unsaved-description = Quitting loses the retouching not yet saved. To keep it, cancel, make room on the Library's disk or reconnect it, then quit again.
# The question's button.
quit-anyway-button = Quit Anyway

## Help ▸ Save Diagnostics… (app/diagnostics.rs).

diagnostics-no-log = This launch of { -app-name } has no log to save.
# The save dialog's title.
diagnostics-save-title = Save Diagnostics
# The file the save dialog suggests, before its .zip. $date: today, as
# 2026-11-11.
diagnostics-file-name = { -app-name } Diagnostics { $date }
# The save dialog's file type.
diagnostics-zip-archive = Zip archive
# $name: the saved file's name; $address: the support email address.
diagnostics-saved = Saved { $name }. Email it to { $address }: it holds the logs and crash reports, so don't attach it to a public issue.

diagnostics-save-failed = The diagnostics couldn't be saved.

## Deptheos ▸ Install Command Line Tool… (app/command_line_tool.rs).

cli-tool-missing = This copy of { -app-name } has no command line tool.
cli-tool-not-installed = Move { -app-name } to the Applications folder and open it from there, then install the command line tool.
# $folder: the folder the command is linked in.
cli-tool-already-installed = The deptheos command is installed in { $folder }. Type "deptheos --help" in Terminal to start.
cli-tool-install-title = Install the command line tool?
# $link: where the command is linked.
cli-tool-install-description = This puts the deptheos command in { $link }, linked to the one inside { -app-name }, so it runs in Terminal and is updated with the app. macOS may ask for an administrator password.
# $link: where the command is linked; $old: what is there now, replaced.
cli-tool-install-description-replacing = This puts the deptheos command in { $link }, linked to the one inside { -app-name }, so it runs in Terminal and is updated with the app. macOS may ask for an administrator password. It replaces the deptheos command there now ({ $old }).
# A paragraph after the description, when the app isn't in an
# Applications folder.
cli-tool-outside-applications = { -app-name } isn't in the Applications folder. If you move or delete it, the deptheos command stops working.
# $link: where the command is linked, holding something not a link to
# the app's command.
cli-tool-taken = There is already a deptheos command at { $link }, and it isn't { -app-name }'s. Remove it, then install the command line tool again.
# Why installing stopped: what is at the link's place changed after the
# question was asked. $link: where the command is linked.
cli-tool-link-changed = What is at { $link } changed after you were asked, so nothing was replaced. Install the command line tool again to see what it would replace.
cli-tool-install = Install
# The administrator password prompt's text.
cli-tool-admin-prompt = { -app-name } wants to install the deptheos command.
cli-tool-installed = The deptheos command is installed. Type "deptheos --help" in Terminal to start.
cli-tool-install-failed = The command line tool couldn't be installed.

## Retouch (app/retouch.rs, paint.rs, app/paint_mode.rs): the title and
## its buttons, the Paint from header and footer over the frame list, and
## the labels over the photo.

# A count of brush strokes. $n: how many.
retouch-strokes = { $n ->
    [one] 1 stroke
   *[other] { $n } strokes
}
# The title while retouching a render. $number: the render's number.
retouch-title-render = Retouching #{ $number }
retouch-title = Retouching
# Under the title. $stack: the stack's name; $strokes: how many strokes.
retouch-title-detail = { $stack } · { $strokes ->
    [one] 1 stroke
   *[other] { $strokes } strokes
}
# The Export button's tooltip while retouching.
retouch-export-tooltip = Export the retouched result
retouch-done = Done
retouch-done-tooltip = Go back to Stacking
# Undo's tooltip. $step: what it takes back ("Paint"); $n: how many steps
# there are to undo.
retouch-undo-tooltip = Undo { $step } · { $n ->
    [one] 1 step
   *[other] { $n } steps
}
# Redo's tooltip. $step: what it puts back ("Paint"); $n: how many steps
# there are to redo.
retouch-redo-tooltip = Redo { $step } · { $n ->
    [one] 1 step
   *[other] { $n } steps
}
retouch-nothing-to-undo = Nothing to undo
retouch-nothing-to-redo = Nothing to redo
retouch-paint-from = Paint from
# Beside Paint from while the frames are listed.
retouch-click-a-frame = Click a frame
# Beside Paint from while the renders are listed.
retouch-click-a-render = Click a render
# The two segments of the switch under Paint from: what the list shows.
retouch-sources-frames = Frames
retouch-sources-frames-tooltip = Paint from a frame of this stack
retouch-sources-renders = Renders
retouch-sources-renders-tooltip = Paint from another render of this stack
# The Renders segment's tooltip, and the list, when there is none.
retouch-sources-renders-none = This stack has no other render to paint from
# Under the frame list, before anything is painted.
retouch-nothing-painted = Nothing painted yet
# Under the frame list. $frames and $renders: how many frames and renders
# the render was painted from.
retouch-painted-from = Painted from · { $renders ->
    [0] { $frames ->
        [one] 1 frame
       *[other] { $frames } frames
    }
   *[other] { $frames ->
        [0] { $renders ->
            [one] 1 render
           *[other] { $renders } renders
        }
       *[other] { $frames ->
            [one] 1 frame
           *[other] { $frames } frames
        } and { $renders ->
            [one] 1 render
           *[other] { $renders } renders
        }
    }
}
# The source picked, by the frame's place in the list. $number: its row.
retouch-source-frame = Frame { $number }
# The source, before one is picked: it opens a menu.
retouch-source-none = Choose
# Over the photo while retouching a render. $number: the render's number.
retouch-painted-render = Painted · #{ $number }
# Over the photo; the key is drawn as a key cap. $key: the key to hold.
retouch-hold-to-see-source = Hold { $key } to see the source

# The source pane's label. $name: the source's name (paint-source-original,
# a frame's file name, a render's label).
paint-source = Source · { $name }
# The eraser's source.
paint-source-original = Original
# A render picked as the source, without a label of its own.
paint-source-render = Render
# The painted pane's label.
paint-painted = Painted

# Why Retouch can't open now: its button's tooltip.
paint-blocked-no-result = Stack first: there is no result to retouch yet
paint-blocked-saving = Retouch opens once the result is saved as a render
paint-blocked-not-saved = The result wasn't saved as a render, so it can't be retouched

