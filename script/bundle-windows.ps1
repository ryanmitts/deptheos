# Build the Windows installer and a portable zip. Both carry the command
# line tool (deptheos.exe) beside the app; the installer can add it to PATH.
#
# Usage: script\bundle-windows.ps1
#
# Needs Visual Studio's C++ tools and Inno Setup 6 (docs/windows.md; both
# are preinstalled on GitHub's Windows runners). Output goes to dist\.
#
# The build is unsigned. To sign it, sign target\release\deptheos-gui.exe and deptheos.exe
# after the build, and pass /DSign plus /Sdeptheos="<sign command> `$f" to iscc so the
# installer and uninstaller are signed too.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

# The workspace version, from [workspace.package].
$inPackage = $false
$version = $null
foreach ($line in Get-Content Cargo.toml) {
    if ($line -match '^\[workspace\.package\]') { $inPackage = $true; continue }
    if ($inPackage -and $line -match '^version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw "no version in [workspace.package]" }

# The sample stacks the app carries (made by script/sample-stack.swift),
# whole, as script/sample-check.sh checks them for the other bundles: a
# release never ships without them by accident. The same list as there
# (folder, frames' prefix, count, digits): keep the two in step.
$samples = @(
    @{ Folder = "hornet"; Prefix = "hornet"; Count = 66; Digits = 2 },
    @{ Folder = "pcb"; Prefix = "pcb"; Count = 200; Digits = 3 }
)
foreach ($sample in $samples) {
    $dir = Join-Path "samples" $sample.Folder
    $format = "$($sample.Prefix)-{0:D$($sample.Digits)}.jpg"
    $missing = 1..$sample.Count | ForEach-Object { $format -f $_ } |
        Where-Object { -not (Test-Path (Join-Path $dir $_)) }
    if ($missing) { throw "the sample stack is incomplete: $dir lacks $($missing -join ', ')" }
    $pattern = "^$($sample.Prefix)-\d{$($sample.Digits)}\.jpg$"
    $extra = Get-ChildItem $dir -File | Where-Object { $_.Name -notmatch $pattern }
    if ($extra) { throw "$dir holds files that aren't its frames: $($extra.Name -join ', ')" }
}

# The tools, found before the long build so a missing one fails at once.
# Inno Setup installs per machine or, with winget --scope user, per user.
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
              "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "Visual Studio not found; see docs/windows.md" }
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "no Visual Studio with the C++ x64 tools; see docs/windows.md" }
$aux = Join-Path $vs "VC\Auxiliary\Build"
$toolsVersion = (Get-Content -Raw (Join-Path $aux "Microsoft.VCToolsVersion.default.txt")).Trim()
$dumpbin = Join-Path $vs "VC\Tools\MSVC\$toolsVersion\bin\Hostx64\x64\dumpbin.exe"
if (-not (Test-Path $dumpbin)) { throw "dumpbin.exe not found at $dumpbin" }
# mt.exe merges the long-path setting into the app's manifest. It is in the
# Windows SDK, which the C++ workload's recommended components include.
$mt = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\mt.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.Directory.Parent.Name -match '^\d+(\.\d+)+$' } |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $mt) { throw "mt.exe not found: add a Windows SDK in Visual Studio Installer (docs/windows.md)" }
$redistVersionFile = Join-Path $aux "Microsoft.VCRedistVersion.default.txt"
$redist = $null
if (Test-Path $redistVersionFile) {
    $redist = Join-Path $vs "VC\Redist\MSVC\$((Get-Content -Raw $redistVersionFile).Trim())\x64"
}
if (-not $redist -or -not (Test-Path $redist)) {
    throw "no C++ redistributable files in $vs; add the Visual Studio component " +
        "Microsoft.VisualStudio.Component.VC.Redist.14.Latest (docs/windows.md)"
}

Write-Host "==> Building deptheos-gui and deptheos $version"
cargo build --release --locked -p deptheos_gpui -p deptheos_cli
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$dist = Join-Path (Get-Location) "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$exe = Join-Path (Get-Location) "target\release\deptheos-gui.exe"
$cli = Join-Path (Get-Location) "target\release\deptheos.exe"

# The app's manifest is the UI kit's, which lacks longPathAware, and a second
# manifest fails the link, so the setting is merged into the built exe
# (packaging\windows\long-paths.manifest). Merging again is harmless when
# cargo didn't relink. Signing, when it is set up, comes after this.
Write-Host "==> Making deptheos-gui.exe long-path aware"
& $mt -nologo -manifest packaging\windows\long-paths.manifest "-inputresource:$exe;#1" "-outputresource:$exe;#1"
if ($LASTEXITCODE -ne 0) { throw "mt.exe failed to merge the manifest" }
$merged = Join-Path $env:TEMP "deptheos-gui.manifest"
& $mt -nologo "-inputresource:$exe;#1" "-out:$merged"
if ($LASTEXITCODE -ne 0) { throw "mt.exe failed to read the manifest back" }
$text = Get-Content -Raw $merged
Remove-Item -Force $merged
if ($text -notmatch 'longPathAware[^>]*>\s*true' -or $text -notmatch 'PerMonitorV2') {
    throw "deptheos-gui.exe's manifest lacks longPathAware or the UI kit's settings after the merge"
}

# The Microsoft C++ runtime DLLs the exes load: the C++ libraries need
# vcruntime140 and msvcp140, and LibRaw's OpenMP needs vcomp140. A clean
# Windows may not have them, and the per-user installer cannot run
# Microsoft's vc_redist.x64.exe (it needs admin), so they go next to the
# exe, which Microsoft supports for these files. Every DLL the exe (or a
# copied DLL) imports must be shipped or part of Windows; anything else
# fails the build. This only sees import tables: a DLL loaded by name at
# run time is invisible here, which is what the clean-VM test is for.
Write-Host "==> Collecting the runtime DLLs from $redist"
$redistDlls = @{}
foreach ($file in Get-ChildItem $redist -Directory -Filter "Microsoft.VC*" | Get-ChildItem -Filter *.dll) {
    $redistDlls[$file.Name.ToLowerInvariant()] = $file.FullName
}

function Get-Imports($path) {
    # Both the import and the delay-load lists: names indented on their own line.
    $out = & $dumpbin /nologo /dependents $path
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on $path" }
    foreach ($line in $out) {
        if ($line -match '^\s+(\S+\.dll)\s*$') { $Matches[1].ToLowerInvariant() }
    }
}

# Windows' own ICU, whose version info does not say "Windows".
$windowsExtras = @("icu.dll", "icuuc.dll", "icuin.dll")
function Test-WindowsDll($name) {
    if ($name -match '^(api|ext)-ms-') { return $true }
    $path = Join-Path $env:windir "System32\$name"
    if (-not (Test-Path $path)) { return $false }
    if ($windowsExtras -contains $name) { return $true }
    return (Get-Item $path).VersionInfo.ProductName -match 'Windows'
}

$runtime = Join-Path (Get-Location) "target\windows-runtime"
Remove-Item -Recurse -Force $runtime -ErrorAction SilentlyContinue
New-Item -ItemType Directory $runtime | Out-Null
$queue = [System.Collections.Generic.Queue[string]]::new()
$queue.Enqueue($exe)
$queue.Enqueue($cli)
$seen = @{}
$unknown = @()
while ($queue.Count -gt 0) {
    foreach ($dll in Get-Imports $queue.Dequeue()) {
        if ($seen.ContainsKey($dll)) { continue }
        $seen[$dll] = $true
        # Before the System32 check: Visual Studio puts these there too.
        if ($redistDlls.ContainsKey($dll)) {
            Copy-Item $redistDlls[$dll] $runtime
            $queue.Enqueue($redistDlls[$dll])
            Write-Host "    $dll"
        } elseif (-not (Test-WindowsDll $dll)) {
            $unknown += $dll
        }
    }
}
if ($unknown) {
    throw "the exe needs DLLs that are neither part of Windows nor in the C++ " +
        "redistributable: $($unknown -join ', '). Ship them, or if Windows " +
        "has them, add them to `$windowsExtras in this script."
}

Write-Host "==> Making the installer"
& $iscc "/DAppVersion=$version" "/DSourceExe=$exe" "/DCliExe=$cli" "/DRuntimeDir=$runtime" `
    "/DOutputDir=$dist" packaging\windows\deptheos.iss
if ($LASTEXITCODE -ne 0) { throw "iscc failed" }

Write-Host "==> Making the portable zip"
$stage = Join-Path $dist "Deptheos"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $exe, $cli, THIRD_PARTY_NOTICES.txt $stage
# .txt, as in the installer, so Windows opens them without asking for an app.
Copy-Item LICENSE.md (Join-Path $stage "LICENSE.txt")
Copy-Item TRADEMARKS.md (Join-Path $stage "TRADEMARKS.txt")
Copy-Item (Join-Path $runtime "*.dll") $stage
# The sample stacks, as the installer puts them (packaging\windows\deptheos.iss).
foreach ($sample in $samples) {
    $stageSample = Join-Path $stage "samples\$($sample.Folder)"
    New-Item -ItemType Directory -Force $stageSample | Out-Null
    Copy-Item (Join-Path "samples\$($sample.Folder)" "*.jpg") $stageSample
}
$zip = Join-Path $dist "Deptheos-$version-windows-x64-portable.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item -Recurse -Force $stage

Get-ChildItem $dist -File | ForEach-Object { Write-Host "==> $($_.FullName)" }
