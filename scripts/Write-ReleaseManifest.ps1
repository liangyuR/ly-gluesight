#Requires -Version 7.0
param(
    [string]$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..')),
    [string]$OutputPath,
    [string]$TimingPath,
    [string]$CacheStatusJson = '{}',
    [string]$Status = 'unknown'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (!$OutputPath) { $OutputPath = Join-Path $ProjectRoot 'src-tauri/target/package-reports/release-manifest.json' }
$package = Get-Content -LiteralPath (Join-Path $ProjectRoot 'package.json') -Raw | ConvertFrom-Json
$lock = Get-Content -LiteralPath (Join-Path $ProjectRoot 'packaging/lyflow.lock.json') -Raw | ConvertFrom-Json
$config = Get-Content -LiteralPath (Join-Path $ProjectRoot 'packaging/runtime.json') -Raw | ConvertFrom-Json
$revision = & git -C $ProjectRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot read application source revision.' }
$tools = [ordered]@{}
foreach ($tool in @('node', 'pnpm', 'rustc', 'cargo')) {
    $command = Get-Command $tool -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    $tools[$tool] = 'unavailable'
    if (!$command) { continue }
    $start = [Diagnostics.ProcessStartInfo]::new()
    if ([IO.Path]::GetExtension($command.Source) -in @('.cmd', '.bat')) {
        $start.FileName = $env:ComSpec
        $start.Arguments = '/d /s /c ""' + $command.Source + '" --version"'
    } else {
        $start.FileName = $command.Source
        $start.ArgumentList.Add('--version')
    }
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        if (!$process.Start()) { continue }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (!$process.WaitForExit(10000)) { $process.Kill($true); $process.WaitForExit(); continue }
        if ($process.ExitCode -eq 0) { $tools[$tool] = $stdout.GetAwaiter().GetResult().Trim() }
        $stderr.GetAwaiter().GetResult() | Out-Null
    } finally { $process.Dispose() }
}
$timings = [ordered]@{}
if ($TimingPath -and (Test-Path -LiteralPath $TimingPath)) {
    foreach ($line in Get-Content -LiteralPath $TimingPath) {
        if ($line -notmatch '^([a-z-]+)=(\d+(?:\.\d+)?)$') { throw 'Invalid build stage timing.' }
        $timings[$Matches[1]] = [double]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)
    }
}
$manifest = [ordered]@{
    schemaVersion = 1
    application = [ordered]@{ version = $package.version; sourceRevision = $revision.Trim() }
    lyflow = $lock
    vendor = [ordered]@{ repository = $config.repository; tag = $config.tag; asset = $config.asset; bundleVersion = $config.bundleVersion; mvsVersion = $config.mvsVersion }
    tools = $tools
    caches = $CacheStatusJson | ConvertFrom-Json
    stageSeconds = $timings
    validationStatus = $Status
    generatedAt = [DateTime]::UtcNow.ToString('o')
}
New-Item -ItemType Directory -Path (Split-Path -Parent $OutputPath) -Force | Out-Null
$manifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
Write-Output "Release manifest: $OutputPath"
