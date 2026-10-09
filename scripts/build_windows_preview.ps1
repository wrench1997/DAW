# Developer-only build orchestration. Requires Windows, official Rust 1.99.0,
# Visual Studio C++ Build Tools and Python 3.11+. Never downloads or publishes.
[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Checked([string] $Command, [string[]] $Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit $LASTEXITCODE" }
}
function Read-Checked([string] $Command, [string[]] $Arguments) {
    $Text = (& $Command @Arguments | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit $LASTEXITCODE" }
    return $Text
}

if (-not $IsWindows) { throw "The PREVIEW build requires Windows PowerShell 7+." }
$Repo = Split-Path -Parent $PSScriptRoot
Set-Location $Repo
$Toolchain = "1.99.0-x86_64-pc-windows-msvc"
$Target = "x86_64-pc-windows-msvc"
$Python = (Get-Command python -ErrorAction Stop).Source
Invoke-Checked $Python @("-c", "import sys; assert sys.version_info >= (3, 11), 'Python 3.11+ required'")
if (Read-Checked git @("status", "--porcelain", "--untracked-files=normal")) {
    throw "Commit the reviewed source before building a traceable preview."
}
$Commit = Read-Checked git @("rev-parse", "HEAD")
$Epoch = Read-Checked git @("show", "-s", "--format=%ct", "HEAD")
$LockHash = (Get-FileHash Cargo.lock -Algorithm SHA256).Hash.ToLowerInvariant()

# Use the installed official VS toolchain; do not download/copy runtime DLLs.
$VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$VsRoot = Read-Checked $VsWhere @("-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath")
if (-not $VsRoot) { throw "Visual Studio C++ Build Tools were not found." }
# Microsoft recommends Launch-VsDevShell.ps1 for build automation. Calling the
# installed PowerShell entry point avoids cmd.exe's nested path quoting rules.
$DevShell = Join-Path $VsRoot "Common7\Tools\Launch-VsDevShell.ps1"
if (-not (Test-Path -LiteralPath $DevShell -PathType Leaf)) {
    throw "Visual Studio Developer PowerShell launcher is missing."
}
& $DevShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
if ($env:VSCMD_ARG_TGT_ARCH -notin @("x64", "amd64") -or
    $env:VSCMD_ARG_HOST_ARCH -notin @("x64", "amd64")) {
    throw "Visual Studio Developer PowerShell did not select x64 host and target tools."
}
$Linker = Join-Path $env:VCToolsInstallDir "bin\Hostx64\x64\link.exe"
if (-not (Test-Path -LiteralPath $Linker -PathType Leaf)) { throw "MSVC linker is missing." }
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $Linker
if ($env:CARGO_ENCODED_RUSTFLAGS) {
    throw "Remove CARGO_ENCODED_RUSTFLAGS: it overrides the recorded static-CRT flags."
}
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$env:CARGO_INCREMENTAL = "0"
$env:SOURCE_DATE_EPOCH = $Epoch
# Separate output to avoid accidentally packaging the ordinary debug/MSVC lane.
$env:CARGO_TARGET_DIR = Join-Path $Repo "target-preview"
$Out = Join-Path $Repo "artifacts\windows-preview"
if (Test-Path -LiteralPath $Out) { throw "Preview output already exists: $Out" }
New-Item -ItemType Directory -Path $Out | Out-Null
$Rustc = Read-Checked rustc @("+$Toolchain", "--version", "--verbose")
$Cargo = Read-Checked cargo @("+$Toolchain", "--version", "--verbose")
Write-Output $Rustc
Write-Output $Cargo
Write-Output "Source: $Commit; Cargo.lock SHA-256: $LockHash"

Invoke-Checked cargo @("+$Toolchain", "fmt", "--all", "--", "--check")
Invoke-Checked cargo @("+$Toolchain", "test", "--locked", "--all-features", "--all-targets", "--target", $Target)
Invoke-Checked cargo @("+$Toolchain", "clippy", "--locked", "--all-features", "--all-targets", "--target", $Target, "--", "-D", "warnings")
Invoke-Checked cargo @("+$Toolchain", "check", "--locked", "--no-default-features", "--all-targets", "--target", $Target)
Invoke-Checked cargo @("+$Toolchain", "build", "--locked", "--release", "--all-features", "--bins", "--target", $Target)

# JSON contains only selected public build facts, never the environment, user
# profile, entire target tree or raw Cargo metadata with absolute local paths.
$BuildInfo = [ordered]@{
    target = $Target
    toolchain = $Toolchain
    rustflags = $env:RUSTFLAGS
    profile = "release"
    features = @("vst2", "vst3")
    source_commit = $Commit
    source_epoch = [long] $Epoch
    cargo_lock_sha256 = $LockHash
    rustc = $Rustc.Replace("`r`n", "`n")
    cargo = $Cargo.Replace("`r`n", "`n")
    msvc_linker_version = (Get-Item -LiteralPath $Linker).VersionInfo.FileVersion
    msvc_linker_sha256 = (Get-FileHash -LiteralPath $Linker -Algorithm SHA256).Hash.ToLowerInvariant()
    msvc_tools_version = $env:VCToolsVersion
    windows_sdk_version = $env:WindowsSDKVersion
    runner_image = [string] $env:ImageOS
    runner_image_version = [string] $env:ImageVersion
    python = Read-Checked $Python @("--version")
}
$BuildInfoPath = Join-Path $Out "build-info.json"
$BuildInfo | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $BuildInfoPath -Encoding utf8NoBOM
$MetadataPath = Join-Path $Out "cargo-metadata.private.json"
$Metadata = Read-Checked cargo @("+$Toolchain", "metadata", "--locked", "--all-features", "--filter-platform", $Target, "--format-version", "1")
$Metadata | Set-Content -LiteralPath $MetadataPath -Encoding utf8NoBOM
$Binaries = Join-Path $env:CARGO_TARGET_DIR "$Target\release"
Invoke-Checked $Python @("-B", "scripts/package_windows_preview.py", "create", "--repo", $Repo, "--binaries", $Binaries, "--metadata", $MetadataPath, "--build-info", $BuildInfoPath, "--output", $Out)
$Archives = @(Get-ChildItem -LiteralPath $Out -Filter "*.zip" -File)
if ($Archives.Count -ne 1) { throw "Expected exactly one preview ZIP." }
Invoke-Checked $Python @("-B", "scripts/package_windows_preview.py", "verify", $Archives[0].FullName, "--extract-to", (Join-Path $Out "extracted"))
Write-Output "Validated package and extracted its checked whitelist. Helper protocol smoke must still pass."
