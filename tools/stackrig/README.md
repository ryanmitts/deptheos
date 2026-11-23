# stackrig

Shoots turntable focus-stack sets without a hand on the camera: drive the lens to its near end,
fire the body's own focus bracketing, turn the Revopoint table a fraction of a degree, repeat.
Canon EOS over USB, the table over Bluetooth LE. macOS only, Swift, no dependencies.

The camera is driven with raw PTP commands through macOS's ImageCaptureCore. No Canon SDK is
involved, so there is nothing to download or copy.

## Setup

1. `swift build -c release` in this folder. The binary is `.build/release/stackrig`.
2. Camera on USB-C, Wi-Fi off. Quit any other tethering app: one that shares the camera through the
   same macOS daemon drains the camera's event queue while it runs, so the tool misses files.
   EOS Utility can stay open.
3. Focus bracketing set up in the camera menu the way you shoot it by hand, or let the tool write
   it: `stackrig bracket on --shots 200 --step-width 4`, or `--configure` on `run`.

First run: macOS asks for Bluetooth access on behalf of the terminal app. Allow it.

## Commands

```sh
stackrig probe                   # find both devices, print the focus bracketing settings
stackrig focus near              # drive the lens to the near end stop
stackrig liveview frame.jpg      # save a live view frame (to check framing from the terminal)
stackrig shoot --shots 200       # fire one bracket, count the files as the camera writes them
stackrig bracket on --shots 200 --step-width 4   # write the bracketing settings (or: bracket off)
stackrig turn 0.5                # turn the table half a degree and wait for it to settle
stackrig run --stacks 100 --step 0.5 --shots 200
```

`run` writes `stackrig-<date>.csv` next to where it was started: one line per stack with the
angle, start and end time, file count and download folder, so the frames on the card map to
stacks without counting. Shots stay on the card. Set `STACKRIG_DEBUG=1` to see every PTP
response and camera event. The option list is at the top of `Sources/stackrig/main.swift`.

`--download DIR` sends each shot to the Mac instead of the card, into `DIR/s001/`, `DIR/s002/`,
… (one folder per stack; `shoot` uses `DIR/shoot/`). A folder that already has files is refused
before anything connects, so a rerun can't write over shots that exist nowhere else: move them,
pick another folder, or resume with `--start`. The tool sets the camera's capture
destination to the computer (EOS property `0xD11C` = 4, after telling it the Mac's free space
with `0x911A`), copies each shot out of the camera's buffer and confirms it with
`TransferComplete` (`0x9117`), and puts the destination back when the session closes.
`--also-card` keeps the card as the destination and copies each file from there instead.

Copying runs at about 14 MB/s through macOS's camera daemon, in 1 MB pieces (an EOS R refuses
larger ones). A 9.5 MB JPEG takes about 0.7 s, slower than the bracket's frame rate,
so the copy falls behind: with `--also-card` the camera finishes the bracket and the tool catches
up before the table turns; sending to the computer, the camera can only shoot as fast as its
buffer empties. Smaller JPEGs keep the two in step.

## How it works

- Session: ImageCaptureCore finds the camera and opens a PTP session; the tool sends
  `SetRemoteMode(1)` and `SetEventMode(1)`, then polls `GetEvent` for files written and property
  values. `SetEventMode(0)` and the session close hand the body back. The opcodes and property
  codes are the constants in `PTP` (`Sources/stackrig/Camera.swift`).
- Focus reset: `DriveLens` in the largest near step under live view (output to the computer; the
  camera's screen is dark for those few seconds). The body
  does not report when the lens reaches the stop, so the tool compares live view frames every ten
  steps and stops when the picture stops changing. `--near-steps N` sends exactly N steps instead,
  for a scene too featureless to compare (a 10-shot bracket at step width 4 took about 30 steps to
  undo; each step is about 0.12 s). The body's own drive-to-end command (`0x9201`) is tried
  first, and the R5 Mark II refuses it over plain PTP; `--focus evf` skips it, `--focus none`
  leaves the lens where it is.
- Shooting: `RemoteReleaseOn(1, 1)` (half press, no AF), `RemoteReleaseOn(2)`, both released. With
  focus bracketing enabled the body runs the whole sequence. Files are counted from the
  `0xC1B6` records the R5 Mark II puts in its event stream (one per file, handles 0x20 apart); the
  wait gives up only when no file has arrived for `--idle` seconds, since long exposures make a
  bracket take as long as it takes. A bracket started at the far end produces nothing: the body
  needs room to step toward infinity.
- Bracketing settings: EOS property `0xD208`, nine 32-bit words (size, version, on/off, shots,
  step width, exposure smoothing, in-camera composite, trimming, flash interval). The R5 Mark II
  reports version 3.
- Turning: `+CT,TURNANGLE=<deg>;` on GATT characteristic FFE1 of service FFE0 (fractions are
  accepted: 0.5 moves half a degree), then `+QT,CHANGEANGLE;` polled until the answer holds still
  for `--settle` seconds. The table answers with its position since power-on
  (`+DATA=1.5;` or `+OK,TURNANGLE=1.01`).
- ImageCaptureCore delivers its completions on the main run loop, so every wait is a loop of
  short `RunLoop.main` runs.

## Measured on the EOS R5 Mark II (firmware 1.3.1)

- A 10-shot bracket: one file every 2.0 s at the exposure used while testing, 20 s in all.
- Focus reset after that bracket: 30 steps, 4.4 s. Turn and settle: 2.2 s.
- The camera needs a few seconds to reappear after a session closes, so back-to-back commands
  wait up to 20 s for it.
