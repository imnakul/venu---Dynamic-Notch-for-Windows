# Per-user uninstaller for Venu - the counterpart to scripts\install.ps1.
#
#   powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1 -PurgeConfig
#
# Removes the autostart entry, shortcuts and the installed files. Settings in
# %APPDATA%\venu are kept unless -PurgeConfig is passed, so a reinstall comes
# back exactly as it was left.
[CmdletBinding()]
param(
    [switch]$PurgeConfig
)

$ErrorActionPreference = "SilentlyContinue"

Get-Process venu | Stop-Process -Force
Start-Sleep -Milliseconds 500

# Autostart entry, plus the Task Manager approval leftover next to it.
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "Venu"
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" -Name "Venu"

$group = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Venu"
Remove-Item (Join-Path $group "Venu.lnk") -Force
Remove-Item $group -Force
Remove-Item (Join-Path ([Environment]::GetFolderPath("Desktop")) "Venu.lnk") -Force

$dest = Join-Path $env:LOCALAPPDATA "Programs\Venu"
if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }

Write-Host "Venu uninstalled."
if ($PurgeConfig) {
    Remove-Item (Join-Path $env:APPDATA "venu") -Recurse -Force
    Write-Host "Configuration removed (%APPDATA%\venu)."
} else {
    Write-Host "Configuration kept at %APPDATA%\venu (use -PurgeConfig to remove)."
}
