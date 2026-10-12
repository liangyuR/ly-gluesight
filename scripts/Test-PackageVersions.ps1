#Requires -Version 7.0
param(
    [string]$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..')),
    [string]$Tag,
    [switch]$VerifyMainAncestor
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$package = Get-Content -LiteralPath (Join-Path $ProjectRoot 'package.json') -Raw | ConvertFrom-Json
$tauri = Get-Content -LiteralPath (Join-Path $ProjectRoot 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$windows = Get-Content -LiteralPath (Join-Path $ProjectRoot 'src-tauri/tauri.windows.conf.json') -Raw | ConvertFrom-Json
$cargo = Get-Content -LiteralPath (Join-Path $ProjectRoot 'src-tauri/Cargo.toml') -Raw
$lock = Get-Content -LiteralPath (Join-Path $ProjectRoot 'src-tauri/Cargo.lock') -Raw
if ($package.version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw 'Application version must be X.Y.Z.' }
if ($cargo -notmatch '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)') { throw 'Missing Cargo package metadata.' }
$metadata = $Matches[1]
if ($metadata -notmatch '(?m)^name\s*=\s*"([^"]+)"\s*$') { throw 'Missing Cargo package name.' }
$crate = $Matches[1]
if ($metadata -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') { throw 'Missing Cargo package version.' }
$cargoVersion = $Matches[1]
$rootEntries = @([regex]::Matches($lock, '(?ms)^\[\[package\]\]\s*(.*?)(?=^\[\[package\]\]|\z)') | Where-Object { $_.Groups[1].Value -match ('(?m)^name\s*=\s*"' + [regex]::Escape($crate) + '"\s*$') })
if ($rootEntries.Count -ne 1 -or $rootEntries[0].Groups[1].Value -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') { throw 'Missing or ambiguous application entry in Cargo.lock.' }
$lockVersion = $Matches[1]
if ($package.version -ne $tauri.version -or $package.version -ne $cargoVersion -or $package.version -ne $lockVersion) { throw 'package.json, Cargo.toml, Cargo.lock and tauri.conf.json versions must agree.' }
if ($Tag -and ($Tag -notmatch '^v\d+\.\d+\.\d+$' -or $Tag -ne "v$($package.version)")) { throw 'Tag must be vX.Y.Z and match the application version.' }
if ($VerifyMainAncestor) {
    if (!$Tag) { throw 'A release tag is required for main ancestry verification.' }
    & git -C $ProjectRoot show-ref --verify --quiet refs/remotes/origin/main
    if ($LASTEXITCODE -ne 0) { throw 'origin/main is required; fetch the complete repository before release verification.' }
    $tagCommit = & git -C $ProjectRoot rev-parse "$Tag^{commit}"
    if ($LASTEXITCODE -ne 0) { throw 'Release tag does not resolve to a commit.' }
    $headCommit = & git -C $ProjectRoot rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $tagCommit -ne $headCommit) { throw 'Release tag must identify the checked-out commit.' }
    & git -C $ProjectRoot merge-base --is-ancestor $tagCommit origin/main
    if ($LASTEXITCODE -ne 0) { throw 'Release tag must point to a commit in main history.' }
}
if ($windows.bundle.windows.webviewInstallMode.type -ne 'skip') { throw 'WebView2 installation must be skipped.' }
if ($package.packageManager -notmatch '^pnpm@(\d+\.\d+\.\d+)$') { throw 'packageManager must pin pnpm.' }
$pnpmVersion = $Matches[1]
$nodeVersion = (Get-Content -LiteralPath (Join-Path $ProjectRoot '.node-version') -Raw).Trim()
if ($nodeVersion -notmatch '^\d+\.\d+\.\d+$' -or $package.engines.node -ne ">=$nodeVersion") { throw '.node-version and engines.node must agree.' }
$rust = Get-Content -LiteralPath (Join-Path $ProjectRoot 'rust-toolchain.toml') -Raw
if ($rust -notmatch '(?m)^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$') { throw 'Rust toolchain must pin a complete version.' }
$rustVersion = $Matches[1]
$versions = [ordered]@{ application = $package.version; node = $nodeVersion; pnpm = $pnpmVersion; rust = $rustVersion }
if ($env:GITHUB_OUTPUT) {
    foreach ($entry in $versions.GetEnumerator()) { "$($entry.Key)=$($entry.Value)" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8NoBOM }
}
$versions | ConvertTo-Json -Compress | Write-Output
