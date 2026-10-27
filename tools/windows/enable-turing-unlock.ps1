# enable-turing-unlock.ps1 — pre-boot registry companion for the
# nv-uefi-unlocker Turing (TU10x) track.
#
# The Windows Turing driver only accepts a pre-OS FEAT unlock
# (FEAT_PLM open + SS0=0x88888888/SS1=0x8) when running in GSP-RM
# firmware mode (EnableGpuFirmware=1). Ada/40-series does not need
# this (verified: our M4 closed with no registry change), but Turing
# legacy init TDR-loops on the open PLM. Run this ONCE as admin,
# then RESTART (a full restart re-enumerates PCI; resume-from-hibernate
# restores the driver WITHOUT re-initialization, so the setting would
# not take effect and a stale-driver session would TDR on the
# unlocked card).
#
# Usage:  powershell -ExecutionPolicy Bypass -File .\enable-turing-unlock.ps1
#         add -Remove to revert to the stock driver mode.

param(
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'
$Class = 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}'

# Turing PCI device-id ranges (adapter family we ship tu10x assets for):
# 0x1E02..0x1FFF (TU102/TU104/TU106/TU116, incl. 16-series) and
# 0x2182..0x2192 (mobile TU117 etc.).
function Test-TuringDevId([string]$MatchingDeviceId) {
    if ($MatchingDeviceId -notmatch '(?i)ven_10de&dev_([0-9a-f]{4})') { return $false }
    $dev = [Convert]::ToInt32($Matches[1], 16)
    return (($dev -ge 0x1E02 -and $dev -le 0x1FFF) -or
            ($dev -ge 0x2182 -and $dev -le 0x2192))
}

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Run this script from an elevated (Administrator) PowerShell."
    exit 1
}

$found = 0; $changed = 0
Get-ChildItem $Class -ErrorAction SilentlyContinue | ForEach-Object {
    $key = $_
    $props = Get-ItemProperty $key.PSPath -ErrorAction SilentlyContinue
    if (-not $props.MatchingDeviceId -or -not (Test-TuringDevId $props.MatchingDeviceId)) { return }

    $found++
    $desc = $props.DriverDesc
    if ($Remove) {
        if ($null -ne $props.EnableGpuFirmware) {
            Remove-ItemProperty -Path $key.PSPath -Name EnableGpuFirmware
            Write-Host "  [-] $($_.PSChildName) $desc : EnableGpuFirmware removed"
            $changed++
        } else {
            Write-Host "  [=] $($_.PSChildName) $desc : already stock"
        }
    } else {
        if ($props.EnableGpuFirmware -ne 1) {
            Set-ItemProperty -Path $key.PSPath -Name EnableGpuFirmware -Value 1 -Type DWord
            Write-Host "  [+] $($_.PSChildName) $desc : EnableGpuFirmware -> 1"
            $changed++
        } else {
            Write-Host "  [=] $($_.PSChildName) $desc : already 1"
        }
    }
}

if ($found -eq 0) {
    Write-Host "No NVIDIA Turing display adapter found under the display class. Nothing to do."
    exit 0
}

if ($Remove) {
    Write-Host "`n$found Turing adapter(s) seen, $changed reverted to stock driver mode."
} else {
    Write-Host "`n$found Turing adapter(s) seen, $changed set to GSP firmware mode (EnableGpuFirmware=1)."
}
Write-Host ""
Write-Host "IMPORTANT: RESTART to apply (Shutdown -> Restart, or the UEFI unlock USB flow)."
Write-Host "Do NOT rely on hibernate/resume: the driver is restored from the hibernation"
Write-Host "image without re-initialization, so the mode switch would not take effect and"
Write-Host "the pre-OS unlock state would TDR the stale session."
if (-not $Remove) {
    Write-Host ""
    Write-Host "Next: boot the unlock USB, confirm, and let it chain into Windows."
}
