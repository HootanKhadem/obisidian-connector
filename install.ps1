# Installs obsidian-connector on Windows.
#   irm https://raw.githubusercontent.com/HootanKhadem/obisidian-connector/main/install.ps1 | iex
# Errors use `throw`, never `exit`: under `irm | iex`, `exit` would close the user's terminal.
$ErrorActionPreference = 'Stop'

$repo = 'HootanKhadem/obisidian-connector'
$bin = 'obsidian-connector'
$installDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\obsidian-connector' }
$url = "https://github.com/$repo/releases/latest/download/$bin-x86_64-pc-windows-msvc.zip"
$zip = Join-Path ([System.IO.Path]::GetTempPath()) "$bin.zip"

Write-Host "Downloading $bin..."
$downloaded = $true
try {
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
} catch {
    $downloaded = $false
    Write-Host "No prebuilt binary at $url ($($_.Exception.Message))."
}

if ($downloaded) {
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Expand-Archive -Path $zip -DestinationPath $installDir -Force
    Remove-Item $zip

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
        Write-Host "Added $installDir to your PATH (open a new terminal to use it)."
    }
    Write-Host "Installed $installDir\$bin.exe"
} elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
    Write-Host "Building from source with cargo instead (this takes a few minutes)..."
    cargo install --git "https://github.com/$repo" --locked $bin
    if ($LASTEXITCODE -ne 0) {
        throw "cargo install failed with exit code $LASTEXITCODE."
    }
    Write-Host "Installed $bin with cargo (into the cargo bin folder, which is already on your PATH)."
} else {
    throw ("Could not download $bin, and cargo is not installed to build it from source. " +
        "Either no release has been published yet or GitHub is unreachable. " +
        "Install Rust from https://rustup.rs and run this again, or check https://github.com/$repo/releases.")
}

Write-Host ""
Write-Host "Next, connect it to your agent, for example:"
Write-Host "  $bin setup claude-desktop --vault `"My Vault`""
