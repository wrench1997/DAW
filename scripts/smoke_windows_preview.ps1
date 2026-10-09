# Run only the checked, extracted helper. No plugins, GUI or audio devices.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw "The PREVIEW smoke requires Windows PowerShell 7+." }
$Repo = Split-Path -Parent $PSScriptRoot
Set-Location $Repo
$Python = (Get-Command python -ErrorAction Stop).Source
$Out = Join-Path $Repo "artifacts\windows-preview"
$Archives = @(Get-ChildItem -LiteralPath $Out -Filter "*.zip" -File)
if ($Archives.Count -ne 1) { throw "Expected exactly one preview ZIP." }
# Extract anew rather than trusting a previously staged directory.
$Extracted = Join-Path $Out "smoke-extracted"
& $Python -B scripts/package_windows_preview.py verify $Archives[0].FullName --extract-to $Extracted
if ($LASTEXITCODE -ne 0) { throw "Archive verification/extraction failed." }
$PreviousPath = $env:Path
try {
    # No Rust/LLVM/VS PATH fallback. This is still a hosted runner, not a clean VM.
    $env:Path = "$env:SystemRoot\System32;$env:SystemRoot"
    & $Python -B scripts/smoke_vst3_helper.py (Join-Path $Extracted "vst3-host-helper.exe") --timeout 5
    if ($LASTEXITCODE -ne 0) { throw "Extracted PREVIEW helper protocol failed." }
} finally {
    $env:Path = $PreviousPath
}
Get-FileHash -LiteralPath $Archives[0].FullName -Algorithm SHA256
Write-Output "PREVIEW PASS: exact archive, hashes, AMD64 GUI images, OS-only imports, extracted helper protocol."
