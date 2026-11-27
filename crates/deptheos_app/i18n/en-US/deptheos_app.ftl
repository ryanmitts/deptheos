# The shared layer's sentences, in American English: what the desktop, iOS
# and the command line all show (ADR-161). Each message is read through
# `say!` in crates/deptheos_app/src. The wording follows
# docs/terms-and-usage.md.
#
# One message per whole sentence where a sentence stands alone. A clause is
# lower case with no full stop; it is said about a frame after its name, in
# a sentence or in a left-out notice ("Left out IMG_0412.CR3 (…)").
# Arguments are finished text (names already quoted, sizes and dimensions
# already formatted) except a count that chooses a plural, which is a
# number.

-app-name = Deptheos
-dng-converter = Adobe DNG Converter

## Putting text together (words.rs, units.rs).

# Sentences one after another. $first: the sentences so far, each with
# its full stop; $then: the next, with its own.
sentences = { $first } { $then }
# Notice lines that have no full stop of their own, as sentences. $first:
# the lines so far; $then: the next.
notices = { $first }. { $then }
# A list before its last item. $items: the items so far; $next: the next.
list-comma = { $items }, { $next }
# Sizes on disk, in decimal units. $n: the number, formatted ("1.2").
size-kb = { $n } KB
# $n: the number, formatted ("142").
size-mb = { $n } MB
# $n: the number, formatted ("1.2").
size-gb = { $n } GB
# Durations. $seconds: whole seconds.
duration-seconds = { $seconds } s
# $minutes: whole minutes.
duration-minutes = { $minutes } min
# $minutes: whole minutes; $seconds: the seconds past them.
duration-minutes-seconds = { $minutes } min { $seconds } s

## Errors (explain.rs). A reason is one or more full sentences; advice is in
## the imperative; the caller adds the lead ("The stack didn't finish.").

# The reason every fault gives.
fault = { -app-name } ran into an unexpected problem and stopped.
# What a cancel says.
canceled = Stacking was canceled.

# A name in straight double quotes. $name: a file, folder or stack name.
quoted = "{ $name }"
# The end of a list of names. $names: the names before the last, a
# list-comma list; $last: the last name.
names-and = { $names } and { $last }
# A list cut short. $names: the names shown, a list-comma list; $more: how
# many more there are.
names-more = { $names } and { $more } more

# A frame left out by an error with no clause of its own.
clause-unstackable = couldn't be stacked
clause-unstackable-fault = couldn't be stacked, because { -app-name } ran into an unexpected problem
clause-unstacked-stopped = wasn't stacked, because the stack stopped

## Why Stack is off, under it or in the alert when it is pressed
## (frames/list.rs).

# The list has no frames: on the desktop.
stack-problem-empty = Add frames to stack.
# The list has no frames: on iOS, which can take a burst.
stack-problem-empty-ios = Add frames, or take a burst.
stack-problem-too-few = Include at least 2 frames.
# $n: how many included frames are gone, at least 1.
stack-problem-missing = { $n ->
    [one] 1 included frame can't be found.
   *[other] { $n } included frames can't be found.
}
# With RAW Decoder set to Apple: linear DNGs (ProRAW, converted DNGs) stay
# linear and can't stack with JPEG, HEIC or other formats.
stack-problem-mixed-linear-dng = The included frames mix linear DNGs, such as ProRAW, with other formats. Include only those DNGs, or only the others.
# With RAW Decoder set to LibRaw or Adobe DNG Converter.
stack-problem-mixed-raw = The included frames mix RAW files with JPEG, HEIC or other formats. Include only RAW files, or only the others.

no-frames = There are no frames to stack.
no-frames-advice = Add frames, or include the excluded ones.
# $file: the frame's quoted name; $size: its dimensions ("6000 × 4000");
# $first: the first frame's dimensions.
frame-size = { $file } is { $size }, but the stack's first frame is { $first }. Every frame must be the size of the first.
same-size-advice = Check that all come from the same camera and crop mode, that none is already stacked or edited, and that all were converted the same way.
# A color profile that gives itself no name, where a sentence names one.
unnamed-profile = an unnamed color profile
# $file: the quoted name of a RAW frame; $first: the quoted name of the
# first frame, a standard image.
mixed-kinds-raw = { $file } is a RAW or DNG file, but { $first } is a standard image. A stack must be all RAW and DNG files or all standard images, such as JPEG, PNG and TIFF.
# $file: the quoted name of a standard image; $first: the quoted name of
# the first frame, a RAW or DNG file.
mixed-kinds-standard = { $file } is a standard image, but { $first } is a RAW or DNG file. A stack must be all RAW and DNG files or all standard images, such as JPEG, PNG and TIFF.
mixed-kinds-advice = Exclude the frames of the other kind.
mixed-renders-compare = A render of RAW frames and a render of standard images can't be compared.
mixed-renders-paint = A render of RAW frames can't be painted from a render of standard images, or the other way around.
no-shared-frame = The two renders share no frame, so they can't be lined up for comparing.
not-raw = Only a stack of RAW or DNG frames can be exported as DNG.
export-as-tiff = Export it as TIFF instead.
needs-pyramid = Grow sequences can be made only from Pyramid renders.
# $count: how many frames are missing; $names: their quoted names.
missing-frames = { $count ->
    [one] { $count } frame can't be found: { $names }.
   *[other] { $count } frames can't be found: { $names }.
}
# $count: how many frames are missing; $stack: the stack's quoted name;
# $names: the frames' quoted names.
missing-frames-of-stack = { $count ->
    [one] { $count } frame of { $stack } can't be found: { $names }.
   *[other] { $count } frames of { $stack } can't be found: { $names }.
}
# After either. $count: how many frames are missing.
missing-frames-advice = { $count ->
    [one] Check that the drive it is on is connected, and that it wasn't moved or renamed.
   *[other] Check that the drive they are on is connected, and that they weren't moved or renamed.
}
no-stacks = There are no stacks to make a turntable sequence from.
no-common-area = The stacks' cropped areas don't overlap, so they can't make one turntable sequence.
# $needed: the space the sequence needs ("67 GB"); $folder: the folder's
# quoted name; $free: the space free there.
no-space = The sequence needs about { $needed } of disk space, but { $folder } has only { $free } free.
free-space-advice = Free some space, or choose another folder.
# $needed: the space a sample stack's frames take ("64 MB"); $free: the
# space free where they are copied.
sample-no-space = The sample stack needs { $needed } of space, but only { $free } is free.
sample-no-space-advice = Free up some space, then try again.
# $needed: the space the frames' conversions still need ("80 GB"); $free:
# the space free on the disk they are written to.
no-conversion-space = Converting these frames with { -dng-converter } needs about { $needed } of disk space, but the disk { -app-name } keeps the conversions on has only { $free } free.
conversion-space-advice = Free some space, or decode RAW files with LibRaw instead.

# Too many frames were left out. $first: the first one's quoted name;
# $reason: a clause saying why.
left-out-only = { $first } { $reason }.
# $total: how many frames the stack has.
left-out-none-stacked = { $total ->
    [one] The frame couldn't be stacked.
   *[other] None of the { $total } frames could be stacked.
}
# $total: how many frames the stack has, more than one.
left-out-one-stacked = Only 1 of the { $total } frames could be stacked.
# $count: how many frames couldn't be stacked; $total: how many it has.
left-out-some = { $count } of { $total } frames couldn't be stacked.
# $first: the first left-out frame's quoted name; $reason: a clause saying
# why.
left-out-first = The first, { $first }, { $reason }.
left-out-advice = Check that the list holds one focus bracket, shot with the same camera and settings, and exclude any frame that doesn't belong.

# A frame couldn't be read. $subject: its quoted name, or a-frame;
# $clause: what happened to it.
frame-clause = { $subject } { $clause }.
a-frame = A frame
clause-picture = couldn't be read as an image file
clause-dng-read = couldn't be read as a DNG
# A DNG the DNG SDK couldn't read, by what the SDK said.
clause-dng-damaged = couldn't be read as a DNG, because it's damaged or isn't one
clause-dng-not-supported = is a kind of DNG { -app-name } can't read
clause-dng-no-memory = couldn't be read, because there isn't enough memory
# $max: the most megapixels a DNG read takes.
clause-dng-too-large = is too large to read: a DNG can have at most { $max } MP
clause-truncated = is incomplete: the file ends before its image does
truncated-advice = Copy it again from the card or the folder it came from.
# Frames that can't be stacked, refused before the stack starts.
clause-cmyk = is a CMYK image, which can't be stacked
clause-lab = is a Lab color image, which can't be stacked
rgb-advice = Export it from your editor in RGB.
clause-hdr = is an HDR image, which can't be stacked
hdr-advice = Export it from your editor as a standard-range image.
clause-tiff-layout = is a TIFF whose pixels are stored in a way that can't be read
tiff-layout-advice = Save it again from your editor as an RGB or grayscale TIFF of 8, 16 or 32 bits.
# $size: the frame's dimensions.
clause-no-memory = couldn't be read, because there isn't enough memory for a frame of { $size }
close-or-smaller = Close other apps, or stack smaller frames.
# $size: the frame's dimensions; $max: the most megapixels a frame can have.
clause-too-large = is too large to stack ({ $size }); a frame can have at most { $max } MP
clause-dng-channels = is a kind of DNG { -app-name } can't stack, because it doesn't decode to a color image
clause-pixel-layout = is in a pixel format { -app-name } can't stack
pixel-layout-advice = Save it as an RGB file of 8 or 16 bits per channel.
clause-empty = has no pixels
clause-too-large-to-read = is too large to read
clause-raw-decode = couldn't be read as a RAW file

# A slider's name.
setting-smoothness = Smoothness
setting-radius = Radius
setting-smoothing = Smoothing
setting-blend-range = Blend Range
setting-denoise = Denoise
setting-consistency = Consistency
# $setting: the slider's name (setting-*); $min and $max: its range.
out-of-range = { $setting } must be from { $min } to { $max }.

# $size: a frame's dimensions; $first: the first frame's.
core-frame-size = A frame is { $size }, but the stack's first frame is { $first }. Every frame must be the size of the first.
# $size: the frames' dimensions; $min: the shortest side Wavelet takes, in
# pixels.
frame-too-small = Frames of { $size } are too small for Wavelet, which needs at least { $min } px a side.
choose-another-method = Choose another method.
# $size: the result's dimensions.
jpeg-too-large = The result, { $size }, is too large for JPEG, which allows at most 65535 px a side.
export-as-tiff-or-png = Export it as TIFF or PNG instead.
tiff-too-large = The TIFF would be larger than 4 GB, the most a TIFF file can hold.
tiff-too-large-advice = Export it with compression, at 8 bits per channel, or as PNG.
# $format: the file format ("PNG").
encode-failed = The { $format } file couldn't be written.
# $file: the file's quoted name.
symlink-loop = { $file } couldn't be written, because its path has too many symbolic links.
choose-another-folder = Choose another folder.
# $path: the quoted path.
not-a-file = { $path } isn't a file name.
not-a-file-advice = Choose another file name.
image-unreadable = A file couldn't be read as an image file.
image-unwritable = The file couldn't be written.
image-too-large = An image file is too large to read.

# File errors. $file: the file's quoted name.
io-not-found = { $file } can't be found.
io-not-found-any = A file can't be found.
# After a file that can't be found.
not-found-advice = Check that the drive it is on is connected, and that it wasn't moved or renamed.
# $file: the file's quoted name.
io-denied = { -app-name } isn't allowed to open { $file }.
io-denied-any = { -app-name } isn't allowed to open a file it needs.
# $file: the file's quoted name.
io-full = The disk holding { $file } is full.
io-full-any = The disk is full.
# $file: the file's quoted name.
io-read-only = { $file } is on a disk that can't be written to.
io-read-only-any = The disk can't be written to.
# $file: the file's quoted name.
io-other = { $file } couldn't be read or written.
io-other-any = A file couldn't be read or written.
# The same, about one of the Library folder's own files, which the user
# never names.
io-library-not-found = Part of the Library folder can't be found.
io-library-denied = { -app-name } isn't allowed to open the Library folder.
io-library-full = The disk holding the Library folder is full.
io-library-read-only = The Library folder is on a disk that can't be written to.
io-library-other = Part of the Library folder couldn't be read or written.
clause-not-found = can't be found
clause-denied = can't be opened, because { -app-name } isn't allowed to
clause-unreadable = couldn't be read

# The Library's own files, which the user never names.
store-damaged = A file in the Library folder is damaged and couldn't be read.
thumbnail-failed = The file's thumbnail couldn't be made.

# The Library is of a newer format than this version reads: it isn't
# opened, and is left as it is. $version: the version that last used it
# ("0.6.0").
library-newer = This Library was last used by { -app-name } { $version }, which is newer than this version, so this one can't open it. It's left as it is.
library-newer-unknown = This Library was last used by a newer version of { -app-name }, so this one can't open it. It's left as it is.
library-newer-advice = Get the latest version of { -app-name }.
# The Library is open in another copy of the app, which holds its lock.
library-in-use-here = The Library is open in another { -app-name } on this computer.
library-in-use-here-advice = Quit the other { -app-name }, then try again.
# Another computer sharing the drive holds the lock. $version: its app's
# version ("0.3.0"); $computer: that computer's name.
library-in-use-on = The Library is open in { -app-name } { $version } on { $computer }.
# $computer: that computer's name.
library-in-use-on-advice = Quit { -app-name } on { $computer }, then try again.
library-in-use = The Library is open in another copy of { -app-name }.
library-in-use-advice = Quit the other copy, then try again.
# The Library's folder isn't there, nor the folder it is in: its drive
# isn't connected. $folder: the Library folder's quoted name.
library-not-connected = The Library folder, { $folder }, isn't there: the drive it's on isn't connected.
library-not-connected-advice = Connect the drive, then try again.
# The Library's folder is kept by a sync service, which can damage it
# while the app writes it. $folder: the folder's quoted name; $service:
# the service's own name ("iCloud Drive", "Dropbox").
library-synced = The Library folder, { $folder }, is in { $service }, which syncs its files while { -app-name } writes them and can damage the Library.
library-synced-advice = Choose a folder that isn't synced.
# The Library's disk can't lock a file, so nothing could keep two apps
# from writing it at once.
library-no-locks = The Library folder is on a disk that can't keep two apps from changing it at once.
library-no-locks-advice = Choose a Library folder on another disk.

# The folders the Library's frames are in (sources).
# $folder: the folder's quoted name.
sources-not-folder = { $folder } isn't a folder.
# $file: the frame's quoted name.
sources-not-file = { $file } isn't there, or isn't a file.
# $file: the file or folder's quoted name.
sources-not-unicode = { $file } has a name { -app-name } can't keep.
sources-unknown-root = The Library has lost track of the folder some of its frames are in.
# The Library's sources.json, which names the folders every render's
# frames are in, can't be read, so the Library isn't opened. $file: its
# quoted name.
sources-damaged = The Library's list of the folders its frames are in, { $file }, is damaged. The renders in the Library are untouched.
# $file: the same quoted name.
sources-damaged-advice = Put { $file } back from a backup of the Library folder, then open { -app-name } again.
# A copy of a folder into the app was checked and some of its frames
# aren't as they are in the folder. $count: how many; $names: their quoted
# names.
sources-not-copied = { $count ->
    [one] { $count } frame wasn't copied whole: { $names }.
   *[other] { $count } frames weren't copied whole: { $names }.
}

# $file: the DNG's quoted name.
dng-write = { $file } couldn't be written as a DNG.
dng-write-advice = Check that the folder can be written to and has space.
dng-metadata = The RAW frames' camera data couldn't be turned into a DNG.
# The DNG SDK or the JPEG XL codec ran out of memory.
dng-no-memory = There wasn't enough memory to finish.

# $path: the quoted path where the converter was looked for.
converter-not-found = { -dng-converter } isn't at { $path }.
converter-not-found-advice = Install it, set where it is, or decode RAW files with LibRaw instead.
converter-not-here = { -dng-converter } doesn't run on this system.
converter-libraw-advice = Decode RAW files with LibRaw instead.
converter-start = { -dng-converter } couldn't be started.
converter-start-advice = Check that it is installed, or decode RAW files with LibRaw instead.
# $file: the RAW file's quoted name; $time: how long it was given ("5 min").
converter-timed-out = { $file } took longer than { $time } to convert with { -dng-converter }.
# $file: the RAW file's quoted name.
converter-failed = { $file } couldn't be converted by { -dng-converter }.
# $file: the RAW file's quoted name.
converter-no-output = { $file } was left unconverted by { -dng-converter }, which may not know this camera.
converter-no-output-advice = Update { -dng-converter }, or decode RAW files with LibRaw instead.
# $path: the quoted path where the converter was looked for.
clause-converter-not-found = can't be converted, because { -dng-converter } isn't at { $path }
clause-converter-not-here = can't be converted, because { -dng-converter } doesn't run on this system
clause-converter-start = can't be converted, because { -dng-converter } couldn't be started
clause-converter-stopped = wasn't converted, because the stack stopped
# $time: how long it was given ("5 min").
clause-converter-timed-out = took longer than { $time } to convert with { -dng-converter }
clause-converter-failed = couldn't be converted by { -dng-converter }
clause-converter-no-output = was left unconverted by { -dng-converter }, which may not know this camera

# How to start over after the GPU failed: an app, or the command line.
start-again-app = Quit and open { -app-name } again.
start-again-command = Run the command again.
gpu-none = { -app-name } couldn't find a GPU it can use.
gpu-start = The GPU couldn't be started.
gpu-start-advice-desktop = Quit and open { -app-name } again, or choose another GPU in Settings ▸ Advanced.
gpu-lost = The GPU stopped responding or was reset.
gpu-memory = The GPU ran out of memory.
# $size: the frames' dimensions; $needed: the GPU memory they need ("9.1
# GB"); $budget: what the GPU has.
gpu-budget = Frames of { $size } need about { $needed } of GPU memory, but this GPU has { $budget }.
# iOS, where the GPU's memory is the app's and the limit is what the app
# has left, which closing other apps barely changes. $size: the frames'
# dimensions; $needed: the memory they need ("1.9 GB"); $budget: what the
# app may use now.
gpu-budget-ios = Frames of { $size } need about { $needed } of memory, but { -app-name } can use only { $budget } now.
# iOS: a stack stopped partway, before the system would end the app, as
# what the app had left ran low.
gpu-low-memory = The stack stopped because { -app-name } was running out of memory.
# $size: the frames' dimensions; $max: the longest side the GPU takes, in
# pixels.
gpu-side = Frames of { $size } are too large for this GPU, which takes at most { $max } px a side.
# $size: the frames' dimensions.
gpu-buffer = Frames of { $size } are too large for this GPU.
gpu-too-large-advice-desktop = Stack smaller frames, or choose another GPU in Settings ▸ Advanced.
gpu-too-large-advice = Stack smaller frames.

# Alignment. A frame the aligner rejected: frame-clause with $subject the
# first frame or a-frame.
the-first-frame = The first frame
clause-featureless = looks blank or black, with no detail to line the other frames up against
exclude-it = Exclude it.
clause-low-correlation = doesn't look enough like the frame before it to line up
# $percent: how much it overlaps, a whole number.
clause-little-overlap = overlaps the frame before it by only { $percent }%, too little to line up
clause-uncorrelated = doesn't look like the frame before it, or barely overlaps it
clause-flips = would have to be flipped or squashed flat to line up
# $dx and $dy: the move across and up or down, in percent to one decimal;
# $max: the most allowed.
clause-shift = would have to move { $dx }% across and { $dy }% up or down to line up, more than the { $max }% allowed
# $percent: the change, to one decimal; $max: the most allowed.
clause-scale = would have to change size by { $percent }% to line up, more than the { $max }% allowed
# $percent: the stretch, to one decimal; $max: the most allowed.
clause-shear = would have to stretch or slant by { $percent }% to line up, more than the { $max }% allowed
# $degrees: the turn, to one decimal; $max: the most allowed.
clause-rotation = would have to turn { $degrees }° to line up, more than the { $max }° allowed
clause-no-overlap = doesn't overlap the frame before it enough to line up
clause-no-match = doesn't look like the frame before it, and may be blank, black or of another scene
clause-unaligned = couldn't be lined up with the frame before it
# A left-out frame's clause and the short hint after it. $clause: why it
# was left out; $hint: what may help (hint-*).
clause-with-hint = { $clause }; { $hint }
hint-try-grid = try Grid search
hint-wider-grid = wider Grid search limits may line it up
try-grid-advice = Try Grid search, or exclude the frame.
wider-grid-advice = Widen the Grid search limits, or exclude the frame.
exclude-the-frame = Exclude the frame.

## iOS (ios.rs): the sentences only the iOS app shows, made in the bridge.

# How iOS tells a person to report a fault, after the fault sentence.
ios-report-fault = Send Diagnostics… in the Library's Settings sends the details.
ios-no-log-file = { -app-name } has no log file for this launch, so there are no diagnostics to send. Quit and open { -app-name } again, then try again.
ios-device-full = This device is full. Free up space, then try again.
ios-try-again = Try again.
ios-retouched-export-failed = The retouched render couldn't be exported.
ios-save-failed = The render couldn't be saved.
ios-sample-not-copied = The sample stack couldn't be copied into { -app-name }.
# Leads of why the engine didn't start, before why: the app shows it in
# place of Stack and in an alert.
ios-gpu-didnt-start = { -app-name } couldn't start its GPU engine.
ios-library-didnt-open = { -app-name } can't open its Library.

## A batch run (batch/machine.rs). Its progress line, and its closing
## notice: short parts joined by " · ", then the reasons as sentences.

# $n: the batch's stacks started so far; $total: how many it has. The
# stack running follows after " · ".
batch-progress = Batch { $n } of { $total }
# After the progress line and " · ", while the run waits to be resumed.
batch-paused = paused
# After the progress line and " · ", while the next stack waits for the
# last one's export. $stack: its name, quoted.
batch-exporting-stack = Exporting { $stack }
batch-saving = Saving the renders
batch-writing-sequence = Writing the turntable sequence
# $written: the exports written or failed; $total: how many there are.
batch-exporting = Exporting · { $written } of { $total }
# $made: renders made; $total: the stacks the batch had.
batch-made = Batch: { $made } of { $total } made
# $made: renders made before Cancel; $total: the stacks the batch had.
batch-canceled-made = Batch canceled: { $made } of { $total } made
# $made: renders made before a save or an export failed and ended the
# batch; $total: the stacks the batch had. Why follows.
batch-stopped-made = Batch stopped: { $made } of { $total } made
# $count: the stacks that failed.
batch-failed = { $count } failed
# $count: renders exported; $folder: the folder's quoted name.
batch-exported-to = { $count } exported to { $folder }
# $count: renders exported.
batch-exported = { $count } exported
# $count: renders that couldn't be exported.
batch-not-exported = { $count } not exported
batch-sequence-not-written = the turntable sequence wasn't written
# $stack: the stack's name.
batch-left-out-of-sequence = { $stack } left out of the sequence
# The closing notice with why things went wrong. $short: the short parts;
# $why: the reasons, full sentences.
batch-reasons = { $short }. { $why }
# Before why: $stack, the stack's quoted name.
batch-stack-failed = { $stack } didn't finish.
# Before why: $stack, the stack's quoted name.
batch-export-failed = { $stack } couldn't be exported.
# Before why: $stack, the stack's quoted name.
batch-render-unsaved = The render of { $stack } couldn't be saved.
# A batch stack whose render couldn't be saved, before why, on its failed
# tile, which names the stack.
batch-failure-render-unsaved = Its render couldn't be saved.
batch-no-stack-for-sequence = No stack was left for the turntable sequence.
# A batch stack's failed tile, for a stack the batch hadn't made when it
# ended (canceled, stopped by a failure, or the app quit).
batch-not-stacked-yet = Not stacked yet: the batch ended before it got to this stack.

# Why a stack was left out of a turntable sequence, after its name and a
# colon in the manifest; the error's sentences follow the first three.
gap-restack-unsaved = its restack couldn't be saved.
gap-render-unsaved = its render couldn't be saved.
gap-restack-failed = it couldn't be stacked again without its first frame.
gap-scale-off = its scale doesn't match the other stacks, even without its first frame.
# A turntable note. $stack: the stack's name.
note-restacked = { $stack } restacked without its first frame
# The progress line while a turntable run restacks. $stack: the stack's name.
checking-stacks = Checking stacks · restacking { $stack } without its first frame

## A stack's notice (session/stack.rs, session.rs).

# One frame left out, and why. $frame: a left-out-item.
left-out-frame = Left out { $frame }
# Several frames left out. $count: how many; $frames: a left-out-item for
# each, joined by left-out-items.
left-out-frames = { $count ->
    [one] Left out { $count } frame: { $frames }
   *[other] Left out { $count } frames: { $frames }
}
# The left-out items before the next. $items: the items so far; $next: the
# next left-out-item.
left-out-items = { $items }; { $next }
# $name: the frame's file name; $reason: a clause saying why it was left
# out, as kept with the render.
left-out-item = { $name } ({ $reason })
# The line saying the result on screen left frames out, before the button
# that selects them. $count: how many.
left-out-line = { $count ->
    [one] { $count } frame left out
   *[other] { $count } frames left out
}
# Notes about frames as they were read. $count: how many frames.
note-float-clipped = Floating-point frames had values above white or below black, which were clipped ({ $count ->
    [one] { $count } frame
   *[other] { $count } frames
})
# $count: how many frames.
note-over-white = Frames with partial transparency were composited over white ({ $count ->
    [one] { $count } frame
   *[other] { $count } frames
})
# $from: the color profile frames were in; $into: the stack's, the first
# frame's; $count: how many frames.
note-clipped = Frames in { $from } had colors outside { $into }, the stack's color space, which were clipped as they were converted ({ $count ->
    [one] { $count } frame
   *[other] { $count } frames
})
# A stack that aligns and crops whose frames all cover under a quarter of
# the frame, so it is left uncropped. No arguments.
note-uncropped = The frames moved too far to crop to the area they all cover, so the result is uncropped and its edges may show smear

## Stacking methods (stacker.rs).

method-pyramid = Pyramid
method-depth = Depth
method-wavelet = Wavelet
# Under the method's choice: when to use it.
method-pyramid-description = Blends the sharpest detail of every frame at each scale. The all-around choice; an edge in front of a far background can get a soft glow.
method-depth-description = Finds the sharpest frame at each spot, then blends neighbors. Smooth on plain backgrounds and clean beside bright edges; hairs and fine texture come out softer than Pyramid.
method-wavelet-description = Takes each pixel's color whole from one frame. For fine crossing detail; a plain background can come out grainy. Denoise keeps noise in plain areas from being picked up as detail.
# A method's settings as its sliders read them, compactly. The letters are
# the sliders' initials. $smoothness: a slider value, formatted.
method-pyramid-summary = Pyramid · S{ $smoothness }
# $radius, $smoothing, $blend: slider values, formatted.
method-depth-summary = Depth · R{ $radius } S{ $smoothing } B{ $blend }
# $denoise: a slider value, formatted.
method-wavelet-summary = Wavelet · D{ $denoise }
# $denoise: a slider value, formatted; $consistency: a whole number.
method-wavelet-consistent-summary = Wavelet · D{ $denoise } C{ $consistency }

## Alignment methods.

align-ecc = ECC (affine)
align-grid = Grid search (similarity)

## What moving a setting does, one sentence each (stacker.rs, SettingHelp):
## under a slider or picker on the desktop, a section footer on iOS.

setting-smoothness-help = Higher shares each spot among more frames, so noise averages out and fine detail softens.
setting-smoothing-help = Higher evens out plain areas where no frame is clearly the sharpest; lower keeps the outline of a subject against a far background narrow.
setting-denoise-help = Higher removes more noise, and more of the faintest texture with it.
setting-radius-help = Higher decides each spot's frame over a larger area, which is steadier but can soften thin hairs.
setting-blend-range-help = Lower takes most spots from a single frame; higher mixes neighboring frames more, which can look softer.
setting-aligner-help = ECC lines up small shifts, turns and changes in size closely; Grid search finds frames that moved further between shots.
setting-crop-help = On keeps only the area every frame covers; off keeps the whole frame, with softer edges where some frames don't reach.
setting-decoder-help = Changes which decoder reads this stack's camera RAW frames; the two crop and color them differently.

## Frame order (order.rs).

order-name = Name
order-capture-time = Capture time
# Why frames stayed in name order, or how tied times were ordered: a
# clause, no capital or full stop. $count: how many frames.
order-missing-times = { $count ->
    [one] { $count } frame has no capture time, so the frames stay in name order
   *[other] { $count } frames have no capture time, so the frames stay in name order
}
order-mixed-cameras = the frames come from more than one camera, so they stay in name order
order-whole-seconds = the capture times are in whole seconds and several frames share a second, so the frames stay in name order
# Other frames record fractions of a second, these don't and share a
# second with another frame. $count: how many frames lack them.
order-missing-sub-seconds = { $count ->
    [one] { $count } frame records no fraction of a second and shares its second with another, so the frames stay in name order
   *[other] { $count } frames record no fraction of a second and share their second with another, so the frames stay in name order
}
# $count: how many frames share a time.
order-ties = { $count ->
    [one] { $count } frame shares a capture time, so those are ordered by shot number, then name
   *[other] { $count } frames share a capture time, so those are ordered by shot number, then name
}
# A single stack left in name order whose capture times put its frames in
# another order.
order-differs = the frames were shot in a different order than their names
# A note on one stack in a list or a batch. $stack: the stack's name;
# $note: one of the clauses above.
order-on-stack = { $stack }: { $note }
# $stack: the stack's name.
order-differs-from-names = { $stack }: the frames were shot in a different order than their names

## A batch's plan (batch.rs).

split-per-folder = One stack per folder
split-auto-sort = Auto sort
# What a batch is made from: the list alone, with no count.
plan-from-list = The list
# $frames: how many frames in all.
plan-from-list-frames = The list, { $frames ->
    [one] { $frames } frame
   *[other] { $frames } frames
}
# $folders: how many folders besides the list.
plan-from-list-and-folders = The list and { $folders ->
    [one] { $folders } folder
   *[other] { $folders } folders
}
# $folders: how many folders besides the list; $frames: how many frames
# in all.
plan-from-list-and-folders-frames = The list and { $folders ->
    [one] { $folders } folder
   *[other] { $folders } folders
}, { $frames ->
    [one] { $frames } frame
   *[other] { $frames } frames
}
# $folders: how many folders.
plan-from-folders = { $folders ->
    [one] { $folders } folder
   *[other] { $folders } folders
}
# $folders: how many folders; $frames: how many frames in all.
plan-from-folders-frames = { $folders ->
    [one] { $folders } folder
   *[other] { $folders } folders
}, { $frames ->
    [one] { $frames } frame
   *[other] { $frames } frames
}
# What it makes. $from: one of the plan-from messages; $renders: how many
# renders.
plan-to-renders = { $from } → { $renders ->
    [one] { $renders } render
   *[other] { $renders } renders
}
# $from: one of the plan-from messages; $stacks: how many stacks.
plan-to-stacks = { $from } → { $stacks ->
    [one] { $stacks } stack
   *[other] { $stacks } stacks
}
# What it leaves out, each added to the line. $count: how many.
plan-skipped = { $count ->
    [one] { $count } frame skipped
   *[other] { $count } frames skipped
}
# $count: how many folders.
plan-too-few = { $count ->
    [one] { $count } folder with too few frames
   *[other] { $count } folders with too few frames
}
# $count: how many folders.
plan-unreadable = { $count ->
    [one] { $count } folder unreadable
   *[other] { $count } folders unreadable
}
# $line: the summary so far; $part: one of the three above.
plan-and = { $line }, { $part }
# Why a batch won't start: its export folder is a folder of its frames, or
# inside one. $folder: that folder's name, quoted.
batch-export-in-frames = The exports can't go into { $folder }, which holds frames this batch stacks. Choose a folder outside the folders being stacked.
# A batch's header in the Library when it skipped frames. $split: how it
# split its folders (split-*); $count: how many frames it skipped.
plan-label-skipped = { $split } · { $count ->
    [one] { $count } frame skipped
   *[other] { $count } frames skipped
}

## Sequences (sequence.rs).

sequence-grow = Grow sequence
sequence-turntable-full = Turntable sequence · Full
sequence-turntable-grow = Turntable sequence · Grow across stacks
# A sequence's progress line. $phase: one of the three above; $written,
# $total: frames written, of how many.
sequence-progress = { $phase } · frame { $written } of { $total } written
# The same while a turntable replays its stacks. $phase, $written,
# $total: as above; $stack, $stacks: the stack being replayed, from 1, of
# how many.
sequence-progress-stack = { $phase } · stack { $stack } of { $stacks } · frame { $written } of { $total } written
# What a sequence made. $count: how many frames; $folder: the folder's name.
sequence-made = { $count ->
    [one] { $count } frame in "{ $folder }"
   *[other] { $count } frames in "{ $folder }"
}
# A turntable's choice of what to write.
turntable-grow = Grow across stacks
turntable-full = Full
turntable-both = Both

## Retouching (paint/worker.rs). A lead, then why, then for a fault
## details-in-log.

paint-not-shown = The retouching couldn't be shown.
paint-not-saved = The retouching couldn't be saved.
paint-not-opened = The render couldn't be opened for retouching.
paint-undo-unread = The retouching's undo history couldn't be read.
# A render shown outside Retouch whose saved retouching couldn't be read;
# why follows.
paint-shown-without = This render is shown without its retouching.
# Why a render's saved retouching couldn't be read, after a lead such as
# paint-shown-without, paint-not-opened or an export's.
paint-unread-damaged = Its saved retouching is damaged.
paint-unread-unreachable = Its saved retouching couldn't be read just now.
paint-unread-unreachable-advice = Try again in a moment.
# Why a render's saved adjustments couldn't be read for an export, after
# the export's lead.
adjust-export-damaged = Its saved adjustments are damaged.
adjust-export-unreachable = Its saved adjustments couldn't be read just now.
# A notice that stays in Retouch while nothing painted can be saved; one
# of the sentences after it says why.
paint-unsaveable = What you paint on this render can't be saved.
paint-unsaveable-no-render = It isn't in the Library any more.
paint-unsaveable-unreachable = Its saved retouching couldn't be read just now, and saving over it would lose it. To try again, leave Retouch before painting anything and open it again.
paint-unsaveable-unwritable = Its folder in the Library couldn't be written to.
details-in-log = The details are in the log.
paint-fault = { -app-name } ran into an unexpected problem while retouching. The details are in the log.
# Over the photo, when the frame sharpest at a point was asked for and the
# render keeps no record of where its detail came from.
paint-no-detail-record = This render doesn't record where its detail came from. Pick a frame to paint from instead.
# A source to paint from couldn't be loaded; one of the sentences after
# it says what to do.
paint-source-failed = The frame or render to paint from couldn't be loaded.
paint-pick-again = Pick it again.
paint-pick-again-or-another = Pick it again, or pick another frame or render.
# Why a frame or render couldn't be lined up with the render painted, so
# it can't be painted from.
paint-no-shared-frame = The two renders share no frame, so they can't be lined up.
paint-frame-not-lined-up = That frame isn't in this render, and no other render links it to this one, so it can't be lined up. Paint from a frame this render used.
# Why a stroke was refused.
paint-cannot-paint-from = This frame or render couldn't be loaded to paint from. Pick it again, or pick another frame or render.
paint-does-not-line-up = That frame or render doesn't line up with this render, so it can't be painted from.
paint-too-many-sources = A render can be painted from at most 255 frames and renders. Paint from one already used.
# What Undo and Redo name a stroke by.
paint-undo-erase = Erase
paint-undo-paint = Paint

## A render's name, wherever either app names one (history.rs,
## render_name): its number in its stack, the star right after the number,
## then the name it was given. Never a method letter, and a stack's only
## render is numbered too.

# $number: the render's number.
render-number = #{ $number }
# A starred render. $number: the render's number.
render-number-starred = #{ $number } ★
# $number: the render's number; $name: the name it was given.
render-number-named = #{ $number } · { $name }
# A starred render. $number: the render's number; $name: the name it was
# given.
render-number-named-starred = #{ $number } ★ · { $name }
# A render in a menu of its stack's renders. $render: the render as named
# above ("#4 ★ · sharper"); $summary: its method and settings ("Pyramid ·
# S4").
render-with-summary = { $render } · { $summary }
# A render as a screen reader says it, where "#4" would be read as "number
# 4". $number: the render's number.
render-spoken = Render { $number }
# $number: the render's number; $method: its method ("Pyramid").
render-spoken-method = Render { $number }, { $method }
# A starred render. $number: the render's number; $method: its method
# ("Pyramid").
render-spoken-method-starred = Render { $number }, { $method }, starred
# $number: the render's number; $name: the name it was given; $method: its
# method ("Pyramid").
render-spoken-named-method = Render { $number }, { $name }, { $method }
# A starred render. $number: the render's number; $name: the name it was
# given; $method: its method ("Pyramid").
render-spoken-named-method-starred = Render { $number }, { $name }, { $method }, starred

## Adding frames to a stack (list/add.rs), on both apps: what a listing
## of images and folders came to.

# The lead of the error when an added folder can't be listed. $folder:
# the folder's name.
add-folder-unreadable = "{ $folder }" couldn't be read.
# An added folder the system won't let Deptheos read; advice follows.
# $folder: the folder's name.
add-folder-not-allowed = { -app-name } isn't allowed to read "{ $folder }".
add-refused-other-stack = The frames weren't added, because another stack was opened while they were read.
add-refused-stacking = The frames weren't added, because a stack started while they were read.
add-nothing = No frames were added. The files are already in the list, or aren't files { -app-name } reads.
# Frames added to a stack that had frames, over New Stack Instead. $n:
# how many frames were added; $name: the stack's name.
add-added = Added { $n ->
        [one] 1 frame
       *[other] { $n } frames
    } to { $name }.

## The Library (library.rs): its count line, Jump To and a tile's badge, on
## both apps.

# The count line under All. $stacks: how many stacks; $renders: how many
# renders.
library-count = { $stacks ->
    [0] No stacks
    [one] 1 stack
   *[other] { $stacks } stacks
} · { $renders ->
    [0] No renders
    [one] 1 render
   *[other] { $renders } renders
}
# The same with what the renders take on disk. $stacks: how many stacks;
# $renders: how many renders; $size: their size ("4.2 GB").
library-count-size = { $stacks ->
    [0] No stacks
    [one] 1 stack
   *[other] { $stacks } stacks
} · { $renders ->
    [0] No renders
    [one] 1 render
   *[other] { $renders } renders
} · { $size }
# The count line under Starred. $renders: how many starred renders;
# $stacks: how many stacks they are in.
library-count-starred = { $renders ->
    [0] No starred renders
    [one] 1 starred render in 1 stack
   *[other] { $renders } starred renders in { $stacks ->
        [one] 1 stack
       *[other] { $stacks } stacks
    }
}
# The count line under Failed. $stacks: how many failed stacks.
library-count-failed = { $stacks ->
    [0] No failed stacks
    [one] 1 failed stack
   *[other] { $stacks } failed stacks
}
# Jump To's spans of days.
library-span-today = Today
library-span-yesterday = Yesterday
library-span-last-week = Previous 7 days
library-span-earlier = Earlier
# A tile's count badge under a filter or search. $shown: how many renders
# it lets through; $total: how many the stack has.
library-badge-of = { $shown } of { $total }

## Recently Deleted, on both apps (history/deleted.rs, ADR-140): what a
## delete moved there, its list, and emptying it.

# A row of the list, or what a notice names: a whole stack. $stack: its
# name.
deleted-stack = { $stack }
# One render of a stack. $stack: the stack's name; $number: the render's
# number.
deleted-render = { $stack } #{ $number }
# Several renders of a stack. $stack: the stack's name; $renders: how
# many.
deleted-renders = { $renders ->
    [one] 1 render of { $stack }
   *[other] { $renders } renders of { $stack }
}
# Failed runs of a stack, with no render. $stack: the stack's name;
# $failed: how many.
deleted-failed = { $failed ->
    [one] Failed stack { $stack }
   *[other] { $failed } failed runs of { $stack }
}
# Renders and failed runs of a stack. $stack: the stack's name; $renders:
# how many renders; $failed: how many failed runs.
deleted-renders-and-failed = { $renders ->
    [one] 1 render
   *[other] { $renders } renders
} and { $failed ->
    [one] 1 failed run
   *[other] { $failed } failed runs
} of { $stack }
# The notice after a delete, with Undo beside it. $what: what went, as a
# row names it ("Bee #2").
deleted-notice-one = { $what } moved to Recently Deleted.
# The notice after a delete of several whole stacks. $stacks: how many.
deleted-notice-stacks = { $stacks ->
    [one] 1 stack moved to Recently Deleted.
   *[other] { $stacks } stacks moved to Recently Deleted.
}
# The notice after a delete reaching several stacks' renders. $renders:
# how many renders and failed runs went.
deleted-notice-renders = { $renders ->
    [one] 1 render moved to Recently Deleted.
   *[other] { $renders } renders moved to Recently Deleted.
}
# A row's days left before it goes for good. $days: how many.
deleted-days-left = { $days ->
    [one] 1 day left
   *[other] { $days } days left
}
# The list with nothing in it.
deleted-none = Nothing was deleted in the last 30 days.
# What the list holds, under its title. $items: how many rows; $size: what
# they take, formatted ("1.2 GB").
deleted-summary = { $items ->
    [one] 1 item · { $size }
   *[other] { $items } items · { $size }
}
# Empty Recently Deleted's question: its title.
empty-deleted-title = Empty Recently Deleted?
# Its text. $items: how many rows go; $size: what they free, formatted.
empty-deleted-message = { $items ->
    [one] The item in Recently Deleted is deleted for good, freeing { $size }. Your frames and exported files are left alone. This can't be undone.
   *[other] The { $items } items in Recently Deleted are deleted for good, freeing { $size }. Your frames and exported files are left alone. This can't be undone.
}
# Its confirming button.
empty-deleted-button = Empty
# What leads the reason when a restore stopped part way.
restore-failed = Not everything could be restored.
# What leads the reason when emptying stopped part way.
empty-deleted-failed = Not all of Recently Deleted could be emptied.

## Delete Library (Settings), on both apps: its question, and how it ended.

# The confirmation's title.
delete-library-title = Delete the Library?
# The confirmation's text. $stacks: how many stacks; $renders: how many
# renders.
delete-library-message = { $stacks ->
    [one] 1 stack
   *[other] { $stacks } stacks
} and { $renders ->
    [0] no renders
    [one] 1 render
   *[other] { $renders } renders
} are deleted, with their retouching, any failed stacks, the working version and everything in Recently Deleted. Your frames and exported files are left alone. This can't be undone.
# The same, naming the Library's folder. $stacks: how many stacks;
# $renders: how many renders; $folder: the folder's path.
delete-library-message-in = { $stacks ->
    [one] 1 stack
   *[other] { $stacks } stacks
} and { $renders ->
    [0] no renders
    [one] 1 render
   *[other] { $renders } renders
} in "{ $folder }" are deleted, with their retouching, any failed stacks, the working version and everything in Recently Deleted. Your frames and exported files are left alone. This can't be undone.
# The confirmation's button.
delete-library-button = Delete Library
# What Delete Library frees. $size: the space, formatted ("4 MB").
delete-frees = This frees { $size }.
# The notice once it is done.
library-deleted = The Library was deleted.
# A delete of the Library that stopped part way, before why.
library-delete-failed = Not all of the Library could be deleted.

## How a working version differs from the render it started from
## (stacks.rs, Changes). A frame change is a shot added, removed,
## included or excluded, the other file of RAW/JPEG pairs, or another
## frame order, so the full forms say "changes to the frames", never a
## count of frames.

# Short, on tiles and list rows. $count: how many changes in all;
# $number: the render's number.
changes-since = { $count ->
    [one] 1 change
   *[other] { $count } changes
} since #{ $number }
# The change line, by the working version's controls with Revert, when
# only the frames changed. $frames: how many changes;
# $number: the render's number.
changes-since-frames = { $frames ->
    [one] 1 change
   *[other] { $frames } changes
} to the frames since #{ $number }
# The change line, by the working version's controls with Revert, when
# only the settings changed. $settings: how many
# changes; $number: the render's number.
changes-since-settings = { $settings ->
    [one] 1 change
   *[other] { $settings } changes
} to the settings since #{ $number }
# The change line, by the working version's controls with Revert, when
# both changed. $settings: how many changes to the
# settings; $frames: how many to the frames (a number, at least 1);
# $number: the render's number.
changes-since-both = { $settings ->
    [one] 1 change
   *[other] { $settings } changes
} to the settings and { $frames } to the frames since #{ $number }
# On the base render's tile, whose title already names it, when the
# working version is edited. $count: how many changes in all.
changes = { $count ->
    [one] 1 change
   *[other] { $count } changes
}

## How the working version differs from a render shown that isn't its base
## (stacks.rs, Showing), counted as the changes above are, so "in the
## frames" never counts frames.

# The change line when only the frames differ. $frames: how many
# differences; $number: the shown render's number.
differences-from-frames = { $frames ->
    [one] 1 difference from #{ $number } in the frames
   *[other] { $frames } differences from #{ $number } in the frames
}
# The change line when only the settings differ. $settings: how many
# differences; $number: the shown render's number.
differences-from-settings = { $settings ->
    [one] 1 difference from #{ $number } in the settings
   *[other] { $settings } differences from #{ $number } in the settings
}
# The change line when both differ. $settings: how many differences in the
# settings; $frames: how many in the frames (a number, at least 1);
# $number: the shown render's number.
differences-from-both = { $settings ->
    [one] 1 difference from #{ $number } in the settings and { $frames } in the frames
   *[other] { $settings } differences from #{ $number } in the settings and { $frames } in the frames
}
# The change line when the working version is the shown render's frames
# and settings again. $number: the shown render's number.
same-as-render = Same frames and settings as #{ $number }

## Revert to a render, when it would lose what hasn't been stacked
## (stacks.rs, Revert): the confirmation's title and body.

# The confirmation's title. $number: the render reverted to.
revert-title = Revert to #{ $number }?
# The body, when the working version has changes since its base. $count:
# how many changes; $base: the base render's number.
revert-loses-changes = { $count ->
    [one] Your 1 change since #{ $base } hasn't been stacked, and will be lost. Every render is kept.
   *[other] Your { $count } changes since #{ $base } haven't been stacked, and will be lost. Every render is kept.
}
# The body, when the working version has no base render.
revert-loses-unstacked = Your frames and settings haven't been stacked, and will be lost. Every render is kept.
# The body, when the working version's base render isn't in the records, so
# its changes can't be counted.
revert-loses-uncounted = Any changes you haven't stacked will be lost. Every render is kept.

## Render Info: a render's record, read-only (stacks.rs, render_setup).
## Each row is a label and a value; a row whose value the working version
## has otherwise says so under it.

# When it was stacked, under its thumbnail. $date: the date and time,
# formatted.
render-stacked-at = Stacked { $date }
# Under a row the working version has another value for. $value: the
# working version's value, as the row's value is worded.
render-setup-working = Working version: { $value }
# Under a frame's row, when the working version no longer lists it.
render-setup-not-in-working = Not in the working version
setup-method = Method
setup-alignment = Alignment
setup-align-frames = Align frames
setup-aligner = Aligner
setup-crop = Crop to the area every frame covers
setup-raw = RAW
setup-decoder = RAW decoder
setup-frames = Frames
setup-left-out = Left out
setup-on = On
setup-off = Off
# The Frames section's first row: how many frames were included.
setup-included = Included
# Its value. $included: how many it included; $total: how many were
# listed (both formatted).
setup-included-of = { $included } of { $total }
setup-order = Order
# A frame's row value, for a frame the render didn't include.
setup-excluded = Excluded
# A frame's row value, for a frame it did include, when only the working
# version's differs.
setup-frame-included = Included
# Which file of a RAW + JPEG pair was stacked.
setup-pair-raw = RAW
setup-pair-jpeg = JPEG

## The preferences both apps offer (preferences.rs): each one's label and
## what it does, under it on the desktop and as its footer on iOS.

pref-jpeg-quality = JPEG quality
pref-jpeg-quality-help = 1–100. Below about 90, JPEG adds ringing around fine edges.
pref-tiff-compression = TIFF compression
# No compression, in the TIFF compression choice (ZIP and LZW are names).
pref-tiff-none = None
pref-tiff-compression-help = All three are lossless. ZIP makes the smallest files; LZW is read by the oldest software; None is the fastest to write and open, and the largest.
pref-file-name = File name
# The pattern's placeholders are written as typed, in braces.
pref-file-name-help = {"{"}stack{"}"} becomes the stack's name, {"{"}render{"}"} the render's number, {"{"}method{"}"} the method and {"{"}date{"}"} the day; the rest stays as typed.
# With the file name pattern. $name: the file name the pattern makes for
# a sample render ("bee-4.jpg").
pref-file-name-example = For example: { $name }
# The stack name the file name example uses, as a sample.
pref-file-name-sample-stack = bee
pref-grid-limits = Grid search limits
pref-grid-limits-help = How far Grid search looks from one frame to the next. Wider limits handle more movement but take longer and risk a wrong match. Recorded with each render.
pref-grid-horizontal = Horizontal shift (%)
pref-grid-vertical = Vertical shift (%)
# Under both shift limits.
pref-grid-shift-help = Of the frame's longer side.
pref-grid-rotation = Rotation (°)
pref-grid-rotation-help = 0 turns the rotation search off.
pref-grid-magnification = Magnification (%)
pref-grid-magnification-help = 0 turns the magnification search off.
pref-exposure-compensation = Exposure compensation
pref-exposure-compensation-help = Scale each frame to the first frame's brightness. For stacks shot at varying exposure only: on a normal rail stack it measures vignetting, not exposure, and darkens the result.

## Adjust's panel (adjust/sliders.rs): each group's heading, each
## adjustment slider's name and what moving it does, one sentence each,
## under it or as its tooltip on the desktop, a section footer on iOS.

adjust-group-white-balance = White Balance
adjust-group-tone = Tone
adjust-group-presence = Presence
adjust-group-sharpening = Sharpening
adjust-group-vignette = Vignette
adjust-temperature = Temperature
adjust-tint = Tint
adjust-exposure = Exposure
adjust-contrast = Contrast
adjust-highlights = Highlights
adjust-shadows = Shadows
adjust-whites = Whites
adjust-blacks = Blacks
adjust-clarity = Clarity
adjust-texture = Texture
adjust-vibrance = Vibrance
adjust-saturation = Saturation
# Sharpening's own slider, under the Sharpening heading.
adjust-sharpening-amount = Amount
adjust-sharpening-radius = Radius
adjust-sharpening-detail = Detail
adjust-sharpening-masking = Masking
# The vignette's own slider, under the Vignette heading.
adjust-vignette-amount = Amount
adjust-vignette-midpoint = Midpoint
adjust-temperature-help = Higher warms the picture toward yellow; lower cools it toward blue.
adjust-tint-help = Higher shifts the picture toward magenta; lower shifts it toward green.
adjust-exposure-help = Higher brightens the whole picture, in stops; bright areas roll off into white rather than clip.
adjust-contrast-help = Higher darkens the dark tones and brightens the bright ones; lower brings both toward the middle.
adjust-highlights-help = Lower brings back detail in bright areas; higher brightens them.
adjust-shadows-help = Higher opens up dark areas; lower deepens them.
adjust-whites-help = Higher makes the brightest tones brighter; lower pulls them down from white.
adjust-blacks-help = Lower makes the darkest tones darker; higher lifts them from black.
adjust-clarity-help = Higher strengthens contrast across broad areas so shapes stand out; lower softens it.
adjust-texture-help = Higher brings out fine surface detail; lower smooths it, leaving larger edges as they are.
adjust-vibrance-help = Higher strengthens muted colors more than strong ones; lower mutes them.
adjust-saturation-help = Higher strengthens every color alike; lower takes color out, to gray at the far left.
adjust-sharpening-amount-help = Higher sharpens edges more; a stacked result is already sharp, so a little goes a long way.
adjust-sharpening-radius-help = Higher sharpens a wider band around each edge; lower keeps it to the finest detail.
adjust-sharpening-detail-help = Higher sharpens the finest detail more; lower holds back halos along large edges.
adjust-sharpening-masking-help = Higher sharpens only the stronger edges, leaving smooth areas such as a plain background alone.
adjust-vignette-amount-help = Lower darkens the edges of the picture; higher lightens them.
adjust-vignette-midpoint-help = Lower starts the vignette nearer the center; higher keeps it to the corners.
# A slider's value in stops of exposure. $value: the number, signed and
# formatted ("+0.35").
adjust-value-stops = { $value } EV
# A temperature in kelvins. $value: the number, whole ("5200").
adjust-value-kelvin = { $value } K
# A size in pixels of the picture. $value: the number, formatted ("1.0").
adjust-value-pixels = { $value } px

## The white balance eyedropper (adjust/eyedropper.rs): why a click set
## nothing, shown as a notice.

adjust-eyedrop-outside = Click on the picture to set the white balance from it.
adjust-eyedrop-clipped = That spot is too bright to set the white balance from. Click a gray or white area that isn't blown out.
adjust-eyedrop-dark = That spot is too dark to set the white balance from. Click a brighter gray area.
adjust-eyedrop-no-white = The white balance can't be set from that spot. Click a gray area.

## An Adjust session (adjust/session.rs): why it didn't open or draw, and
## a write that failed, as notices.

adjust-unread-damaged = This render's adjustments were damaged, so they start again from none.
adjust-unread-unreachable = This render's adjustments couldn't be read just now. Try again in a moment.
adjust-no-memory = There isn't enough memory to adjust this render right now. Close other apps, then try again.
adjust-open-failed = The render couldn't be opened for Adjust.
adjust-draw-failed = The render couldn't be drawn with these adjustments.
adjust-write-failed = The adjustments couldn't be saved.
