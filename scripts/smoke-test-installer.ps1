# Smoke-test a Windows NSIS installer: install it silently (per-user), run the
# installed binary's `hook` CLI path, then uninstall silently and check the
# install is gone. Used by the manually run Windows installer workflow
# (.github/workflows/windows-installer.yml), and runnable locally:
#
#   pwsh scripts/smoke-test-installer.ps1 "backend/target/<triple>/release/bundle/nsis/mAIestro Code_X.Y.Z_<arch>-setup.exe"
#
# It refuses to run over an existing mAIestro Code install, so it never
# clobbers a real one. The hook writes only under a throwaway MAIESTRO_HOME.
param([Parameter(Mandatory)][string]$Installer)

$ErrorActionPreference = 'Stop'

function Get-UninstallEntry {
  Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq 'mAIestro Code' }
}

# Waits up to $Seconds for $Condition to hold; NSIS's uninstaller re-launches
# itself from %TEMP% and returns before it finishes, so polling is required.
function Wait-Until([scriptblock]$Condition, [int]$Seconds, [string]$What) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while (-not (& $Condition)) {
    if ((Get-Date) -gt $deadline) { throw "timed out after ${Seconds}s waiting for: $What" }
    Start-Sleep -Milliseconds 500
  }
}

$Installer = (Resolve-Path $Installer).Path
if (Get-UninstallEntry) { throw 'mAIestro Code is already installed for this user -- refusing to touch a real install' }

Write-Host "Installing $Installer"
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "installer exited $($p.ExitCode)" }

$entry = Get-UninstallEntry
if (-not $entry) { throw 'no uninstall entry under HKCU after install (expected a per-user install)' }
$dir = $entry.InstallLocation.Trim('"')
Write-Host "Installed to $dir"
if (-not $dir.StartsWith($env:LOCALAPPDATA, [StringComparison]::OrdinalIgnoreCase)) {
  throw "install dir $dir is not under %LOCALAPPDATA% -- installMode currentUser not applied?"
}
$exe = Get-ChildItem -Path $dir -Filter *.exe | Where-Object { $_.Name -notlike 'uninstall*' } | Select-Object -First 1
if (-not $exe) { throw "no app binary in $dir" }

# The hook CLI exits before Tauri starts, so it runs on a headless runner. The
# release binary is a GUI-subsystem exe, so Start-Process -Wait (not a plain
# call) is what waits for it.
$home_ = Join-Path ([IO.Path]::GetTempPath()) "maiestro-smoke-$PID"
New-Item -ItemType Directory -Force $home_ | Out-Null
$stdin = Join-Path $home_ 'payload.json'
Set-Content -Path $stdin -Value '{}' -Encoding ascii
$env:MAIESTRO_HOME = $home_
try {
  Write-Host "Running: $($exe.Name) hook idle --workspace smoke-test"
  $h = Start-Process -FilePath $exe.FullName -ArgumentList 'hook', 'idle', '--workspace', 'smoke-test' `
    -RedirectStandardInput $stdin -Wait -PassThru -NoNewWindow
  if ($h.ExitCode -ne 0) { throw "hook exited $($h.ExitCode)" }
  $status = Join-Path $home_ 'status\smoke-test.json'
  if (-not (Test-Path $status)) { throw "hook wrote no status file at $status" }
  $state = (Get-Content $status -Raw | ConvertFrom-Json).state
  if ($state -ne 'idle') { throw "status file has state '$state', expected 'idle'" }
  Write-Host "Hook wrote $status (state: $state)"
} finally {
  Remove-Item Env:MAIESTRO_HOME
  Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
}

$uninstaller = $entry.UninstallString.Trim('"')
Write-Host "Uninstalling via $uninstaller"
Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait | Out-Null
Wait-Until { -not (Test-Path (Join-Path $dir $exe.Name)) } 60 "$($exe.Name) removed"
Wait-Until { -not (Get-UninstallEntry) } 60 'uninstall entry removed'
Write-Host 'Smoke test passed'
