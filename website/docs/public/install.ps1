$ErrorActionPreference = "Stop"

$Repo = "iamvxrn/cbld"
$Binary = "cbld"
$Target = "x86_64-pc-windows-msvc"
$Ext = "zip"

$Tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -ErrorAction SilentlyContinue).tag_name
if ([string]::IsNullOrWhiteSpace($Tag)) {
    $Tag = "v0.5.0"
}

$Url = "https://github.com/$Repo/releases/download/$Tag/$Binary-$Tag-$Target.$Ext"
$InstallDir = "$env:USERPROFILE\.local\bin"

Write-Host "Downloading $Binary $Tag for $Target..."
$TmpDir = Join-Path $env:TEMP ([guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $TmpDir | Out-Null
$ZipPath = Join-Path $TmpDir "$Binary.$Ext"

Invoke-WebRequest -Uri $Url -OutFile $ZipPath

Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

Move-Item -Path (Join-Path $TmpDir "$Binary.exe") -Destination (Join-Path $InstallDir "$Binary.exe") -Force

Remove-Item -Path $TmpDir -Recurse -Force

Write-Host "$Binary successfully installed to $InstallDir\$Binary.exe"
Write-Host "Make sure $InstallDir is in your PATH."
