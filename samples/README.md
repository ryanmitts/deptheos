# The sample stacks

The focus brackets both apps carry and open with Try a Sample Stack
(ADR-151), shot by Ryan Mitts, each at half the camera's size (4096×2732),
JPEG:

| Folder | In the apps | Frames | How it was shot | Size |
|---|---|---|---|---|
| `hornet/` | Hornet | 66 | A bald-faced hornet on a branch, handheld | 64 MB, quality 70 |
| `pcb/` | Circuit Board | 200 | A circuit board, on a tripod with a focus rail | 68 MB, quality 35 |

The frames are made from the full-size sets by `script/sample-stack.swift`,
which keeps the camera, lens, exposure and capture time and drops serial
numbers, the owner's name and everything else:

```sh
swift script/sample-stack.swift <hornet's frames> samples/hornet hornet
swift script/sample-stack.swift <the board's frames> samples/pcb pcb 2 0.35
```

`script/sample-check.sh` checks both folders are whole before a bundle is
made; nothing else may be in them. Full-size sets are on the website
(Getting started, "A stack to try").

The photographs are all rights reserved and are not under the GPL; they
may be distributed unmodified with Deptheos (LICENSE.md).
