# Windows

## Setting up a build machine

1. **Visual Studio's C++ tools**: Visual Studio 2022 or newer with the
   *Desktop development with C++* workload. Community is free for
   individuals and open source; Build Tools is the same compiler without
   the IDE. From the command line:

   ```
   winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
   ```

   The workload's recommended components include the C++ redistributable
   files (`Microsoft.VisualStudio.Component.VC.Redist.14.Latest`), which
   the installer ships. Without them `script\bundle-windows.ps1` stops and
   names that component; add it with Visual Studio Installer → Modify →
   Individual components.
2. **Rust**: `winget install Rustlang.Rustup`, then `rustup default stable`
   (the default `x86_64-pc-windows-msvc` toolchain).
3. **LibRaw**: `git submodule update --init`.
4. **Inno Setup 6**, for the installer only: `winget install JRSoftware.InnoSetup`.

`cargo run --release -p deptheos_gpui` runs the app. `script\bundle-windows.ps1`
builds the installer and the portable zip into `dist\` (see
[releasing.md](releasing.md)).

## Paths over 260 characters

Both exes are long-path aware, so where Windows' `LongPathsEnabled`
setting is on, frames in deeply nested folders open. `deptheos.exe` gets
the setting from its own manifest (`crates/deptheos_cli/windows.manifest`,
embedded by its `build.rs`). `deptheos-gui.exe` can't: the UI kit embeds
its own manifest and a second one fails the link, so
`script\bundle-windows.ps1` merges `packaging\windows\long-paths.manifest`
into the built exe with the Windows SDK's `mt.exe` and checks the result.
An app run with `cargo run` doesn't have it; only the bundled one does.
Anything that signs the exe has to come after the merge.

## The runtime DLLs

The C++ libraries (LibRaw, the DNG SDK, libjxl) need Microsoft's C++
runtime, `vcruntime140.dll`, `vcruntime140_1.dll` and `msvcp140.dll`, and
LibRaw's OpenMP threading needs `vcomp140.dll`. A clean Windows may not
have them, so the installer and the zip put them next to the exe (Microsoft
supports this "app-local" deployment for these files). Users install
nothing extra.

`script\bundle-windows.ps1` copies them from Visual Studio's redistributable
folder, and fails if the exe needs any other DLL that is not part of
Windows. The copies are whatever version that Visual Studio has, and
Windows Update does not patch them, so update Visual Studio before a
release.

The check reads only the exe's import table. A DLL loaded by name at run
time does not show up there, and only a clean machine catches it.

## Testing the installer on a clean Windows

Your own machine is not a test: Visual Studio has put the runtime DLLs in
`System32`, so a missing DLL works there anyway. GitHub's runners have
Visual Studio too.

### A Hyper-V VM

Set it up once (Windows 11 Pro; virtualization on in the BIOS):

1. Turn on Hyper-V in an admin PowerShell, then reboot (or Turn Windows
   features on or off → Hyper-V):

   ```
   Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
   ```

2. Download the Windows 11 Enterprise evaluation ISO (free for 90 days,
   Microsoft Evaluation Center).
3. Hyper-V Manager → New → Virtual Machine: Generation 2, the Default
   Switch, the ISO. Then in the VM's Settings: Memory 16 GB with Dynamic
   Memory off, 4 or more processors, and Security → Enable Trusted
   Platform Module (Windows 11 needs it).
4. Install Windows in it, sign in, and take a checkpoint (Action →
   Checkpoint) named "clean".

The VM has no GPU, only Windows' software renderer, as on a PC without a
graphics card. Its GPU memory is the VM's RAM: a 45 MP pyramid stack needs
about 5 GB of it, depth about 4.7 GB, on top of Windows and the app.

Each test:

1. `script\bundle-windows.ps1`.
2. Revert the VM to the "clean" checkpoint and start it.
3. Copy the installer, the zip and a stack of RAW frames in, from an admin
   PowerShell on your PC (Enhanced Session's copy and paste works too,
   where it connects):

   ```
   $vm = "Deptheos Test"
   Enable-VMIntegrationService -VMName $vm -Name "Guest Service Interface"
   Get-ChildItem dist, D:\path\to\a\raw\stack -File | ForEach-Object {
       Copy-VMFile $vm -SourcePath $_.FullName -DestinationPath "C:\Test\$($_.Name)" -CreateFullPath -FileSource Host
   }
   ```

   Not to `C:\Users\Public\Desktop`: it shows on the desktop, but it is a
   hidden folder, so the app's file dialog does not list it.

Then check:

- [ ] The installer runs without an admin prompt. (Unsigned, SmartScreen
      warns first: More info → Run anyway.)
- [ ] The app starts. A missing DLL shows "The code execution cannot
      proceed because X.dll was not found".
- [ ] Stacking the RAW frames gives a result. This tests LibRaw with
      OpenMP. On the software renderer a 45 MP stack takes minutes, some
      of them with no progress shown; check Task Manager's CPU column
      before calling it stuck.
- [ ] A stack's render, saved to the Library, opens again (libjxl).
- [ ] Saving a DNG and a TIFF works.
- [ ] Installing a newer build over this one works.
- [ ] Settings → Apps uninstalls it, and `%LOCALAPPDATA%\Programs\Deptheos`
      is gone.
- [ ] The portable zip, extracted, runs too.

The app writes its log to a file per launch in
`%LOCALAPPDATA%\Deptheos\logs` (Help ▸ Show Log Folder opens it), and to
standard error, which a release build has nowhere to show. To watch it
live, run it from PowerShell in the install folder:
`Start-Process .\deptheos-gui.exe -RedirectStandardError C:\Test\log.txt`.

### Another GPU, or none, on your own PC

Settings ▸ Advanced ▸ GPU picks the device the app stacks
on, from the next start: Automatic (a dedicated GPU before an integrated
one), any GPU it found, or Software rendering, Windows' renderer on the
CPU (WARP), as on a PC without a GPU. A desktop Ryzen's integrated
graphics stands in for a laptop's, slower than most. The window itself is
drawn by GPUI on a GPU of its own choosing either way.

`DEPTHEOS_SOFTWARE_GPU=1` forces the software renderer whatever the
setting, and is the only way for `deptheos`. Software rendering is
slow (an 8-frame 45 MP pyramid stack took 277 s on a 32-thread CPU) but
needs no VM.
