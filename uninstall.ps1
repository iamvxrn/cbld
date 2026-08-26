$ErrorActionPreference = "Stop"

$Binary = "cbld"
$InstallDir = Join-Path $env:USERPROFILE ".local\bin"
$Path = Join-Path $InstallDir $Binary
$Exe = Join-Path $InstallDir "$Binary.exe"

if (Test-Path $Exe) {
    Remove-Item -Force $Exe
    Write-Host "$Binary removed from $InstallDir"
} elseif (Test-Path $Path) {
    Remove-Item -Force $Path
    Write-Host "$Binary removed from $InstallDir"
} else {
    Write-Host "$Binary is not installed in $InstallDir"
}
