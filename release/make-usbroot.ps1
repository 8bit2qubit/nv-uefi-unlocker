# make-usbroot.ps1 - regenerate the unlock USB stick root from pinned public
# inputs. NVIDIA firmware blobs are NOT distributed with this project; this
# script downloads the two pinned public packages, verifies their SHA-256,
# and regenerates every file the EFI application needs.
#
# Usage:
#   .\make-usbroot.ps1 -AssetsExe .\assets-x86_64-windows.exe -BootEfi .\BOOTX64.EFI -Out .\usbroot
#
# Requires: PowerShell 5+ and internet access to the two pinned URLs.

param(
    [Parameter(Mandatory = $true)][string]$AssetsExe,
    [Parameter(Mandatory = $true)][string]$BootEfi,
    [string]$Out = ".\usbroot",
    [string]$Work = ".\inputs"
)

$ErrorActionPreference = "Stop"

# Pinned public inputs (see AGENTS.md / assets/src/manifest.rs).
$Sources = @(
    @{
        Name = "NVIDIA-kernel-module-source-610.43.03.tar.xz"
        Url  = "https://download.nvidia.com/XFree86/NVIDIA-kernel-module-source/NVIDIA-kernel-module-source-610.43.03.tar.xz"
        Sha256 = "7E118923C7A23EDC36114D63273A46E3E04E9AF98695A42203E7AC2DFE9FC1DC"
    },
    @{
        Name = "nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst"
        Url  = "https://archive.cachyos.org/archive/cachyos-v3/nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst"
        Sha256 = "E17E7C92EEFFA178B0FE1F40EB17A47095F8FC3CE4A77BF115CF5BD5A4DF86E9"
    }
)

if (-not (Test-Path $AssetsExe)) { throw "assets tool not found: $AssetsExe" }
if (-not (Test-Path $BootEfi))   { throw "BOOTX64.EFI not found: $BootEfi" }
New-Item -ItemType Directory -Force -Path $Work, $Out | Out-Null

foreach ($s in $Sources) {
    $dest = Join-Path $Work $s.Name
    if (-not (Test-Path $dest)) {
        Write-Host "[fetch] $($s.Name)"
        Invoke-WebRequest -Uri $s.Url -OutFile $dest
    }
    $hash = (Get-FileHash -Algorithm SHA256 $dest).Hash
    if ($hash -ne $s.Sha256) {
        throw "SHA-256 mismatch for $($s.Name): got $hash want $($s.Sha256) - refusing (pinned inputs only)"
    }
    Write-Host "[ok] $($s.Name) SHA-256 verified"
}

$SrcTar = Join-Path $Work $Sources[0].Name
$UtilsZst = Join-Path $Work $Sources[1].Name

Write-Host "[extract] AD102 booter / GSP boot / scrubber"
& $AssetsExe extract-ad102 $SrcTar $Out
if ($LASTEXITCODE) { throw "extract-ad102 failed" }
& $AssetsExe extract-scrub $SrcTar $Out
if ($LASTEXITCODE) { throw "extract-scrub failed" }

Write-Host "[extract] GSP firmware images (gsp_ga10x / gsp_tu10x)"
& $AssetsExe fw $UtilsZst $Out
if ($LASTEXITCODE) { throw "fw failed" }

Write-Host "[extract] fwimage / fwsignature sections"
& $AssetsExe gsp-sections (Join-Path $Out "gsp_ga10x.bin") $Out
if ($LASTEXITCODE) { throw "gsp-sections failed" }

Write-Host "[generate] V67 AD102 four-write payload"
& $AssetsExe generate-ad102 (Join-Path $Out "v67-ad102-four-plm.bin")
if ($LASTEXITCODE) { throw "generate-ad102 failed" }

# Assemble the stick root with the exact names the EFI application looks up.
Write-Host "[assemble] $Out"
Copy-Item $BootEfi (Join-Path $Out "BOOTX64.EFI") -Force
# gsp-sections emits fwimage_ad10x.bin / fwsignature_ad10x.bin (the ELF
# section family name); the loader looks the device family up as ad102.
Copy-Item (Join-Path $Out "fwimage_ad10x.bin")     (Join-Path $Out "fwimage_ad102.bin") -Force
Copy-Item (Join-Path $Out "fwsignature_ad10x.bin") (Join-Path $Out "fwsignature_ad102.bin") -Force

$expected = @(
    "BOOTX64.EFI", "booter_load_ad102.bin", "gsp_boot_desc_ad102.bin",
    "gsp_boot_ad102.bin", "fwimage_ad102.bin", "fwsignature_ad102.bin",
    "scrubber_ad10x.bin", "v67-ad102-four-plm.bin"
)
foreach ($f in $expected) {
    if (-not (Test-Path (Join-Path $Out $f))) { throw "missing: $f" }
    Write-Host ("  {0,-28} {1,10} bytes" -f $f, (Get-Item (Join-Path $Out $f)).Length)
}
Write-Host "`nDONE. Copy the CONTENTS of $Out to a FAT32 USB stick root."
Write-Host "Unlock mode requires v67-ad102-four-plm.bin on the stick;"
Write-Host "removing it switches the EFI into replay/diagnostic mode."
