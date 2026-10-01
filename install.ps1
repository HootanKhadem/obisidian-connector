# Installs obsidian-connector on Windows.
#   irm https://raw.githubusercontent.com/HootanKhadem/obisidian-connector/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$repo = 'HootanKhadem/obisidian-connector'
$bin = 'obsidian-connector'
$installDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\obsidian-connector' }
$url = "https://github.com/$repo/releases/latest/download/$bin-x86_64-pc-windows-msvc.zip"

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$zip = Join-Path ([System.IO.Path]::GetTempPath()) "$bin.zip"

Write-Host "Downloading $bin..."
Invoke-WebRequest -Uri $url -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $installDir -Force
Remove-Item $zip

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
    Write-Host "Added $installDir to your PATH (open a new terminal to use it)."
}

Write-Host "Installed $installDir\$bin.exe"
Write-Host ""
Write-Host "Next, connect it to your agent, for example:"
Write-Host "  $bin setup claude-desktop --vault `"My Vault`""
