#Requires -Version 7.0
param(
    [Parameter(Mandatory, Position = 0)][string]$Version,
    [string]$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($Version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw 'Application version must be X.Y.Z.' }
$packagePath = Join-Path $ProjectRoot 'package.json'
$package = Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json
$cargoPath = Join-Path $ProjectRoot 'src-tauri/Cargo.toml'
$cargo = Get-Content -LiteralPath $cargoPath -Raw
if ($cargo -notmatch '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)') { throw 'Missing Cargo package metadata.' }
$metadata = $Matches[1]
if ($metadata -notmatch '(?m)^name\s*=\s*"([^"]+)"\s*$') { throw 'Missing Cargo package name.' }
$crate = $Matches[1]
$lockPath = Join-Path $ProjectRoot 'src-tauri/Cargo.lock'
$lock = Get-Content -LiteralPath $lockPath -Raw
$rootEntries = @([regex]::Matches($lock, '(?ms)^\[\[package\]\]\s*(.*?)(?=^\[\[package\]\]|\z)') | Where-Object { $_.Groups[1].Value -match ('(?m)^name\s*=\s*"' + [regex]::Escape($crate) + '"\s*$') })
if ($rootEntries.Count -ne 1) { throw 'Missing or ambiguous application entry in Cargo.lock.' }
$versionPattern = '(?m)^(version\s*=\s*")[^"]+("\s*)$'
if ($metadata -notmatch $versionPattern -or $rootEntries[0].Groups[1].Value -notmatch $versionPattern) { throw 'Missing application version in Cargo manifests.' }
$packageBlock = [regex]::Match($cargo, '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)').Value
$newPackageBlock = [regex]::Replace($packageBlock, $versionPattern, "`${1}$Version`${2}")
$cargo = $cargo.Replace($packageBlock, $newPackageBlock)
$lockEntry = $rootEntries[0].Value
$newLockEntry = [regex]::Replace($lockEntry, $versionPattern, "`${1}$Version`${2}")
$lock = $lock.Replace($lockEntry, $newLockEntry)
$tauriPath = Join-Path $ProjectRoot 'src-tauri/tauri.conf.json'
$tauri = Get-Content -LiteralPath $tauriPath -Raw | ConvertFrom-Json
$package.version = $Version
$tauri.version = $Version
$package | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $packagePath -Encoding utf8NoBOM
$tauri | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $tauriPath -Encoding utf8NoBOM
[IO.File]::WriteAllText($cargoPath, $cargo, [Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText($lockPath, $lock, [Text.UTF8Encoding]::new($false))
& (Join-Path $PSScriptRoot 'Test-PackageVersions.ps1') -ProjectRoot $ProjectRoot
