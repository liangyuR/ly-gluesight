#Requires -Version 7.0
param(
    [string]$Version,
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '..'),
    [string]$CredentialFile
)
. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$RepositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$config = Get-LyFlowDependencyConfig $RepositoryRoot
$versions = if ($Version) { @() } else { @(Get-LyFlowAvailableVersions $RepositoryRoot $config $CredentialFile) }
$selected = Resolve-LyFlowPackageVersion $versions $config.lyflow $Version
$current = Get-LyFlowDependencyLock $RepositoryRoot $config
$cacheRoot = Join-Path $RepositoryRoot 'src-tauri/target/dependency-cache/nuget'
$temporary = Join-Path $cacheRoot ".update-$([guid]::NewGuid().ToString('N'))"
[void][IO.Directory]::CreateDirectory($temporary)
try {
    if ($selected -ceq $current.version) {
        $native = Restore-LyFlowDependency $RepositoryRoot $config $current $CredentialFile
    } else {
        [void](Invoke-LyFlowNuGet $RepositoryRoot $config.lyflow.source @('install', $config.lyflow.packageId, '-Version', $selected, '-Source', 'LyFlow', '-OutputDirectory', $temporary, '-ExcludeVersion', '-DirectDownload', '-NoCache', '-PackageSaveMode', 'nupkg') $CredentialFile)
        $native = Join-Path $temporary "$($config.lyflow.packageId)/runtimes/win-x64/native"
    }
    $descriptor = Get-Content -LiteralPath (Join-Path $native 'lyflow-runtime.json') -Raw | ConvertFrom-Json
    $locked = [pscustomobject][ordered]@{ schemaVersion = 1; packageId = $config.lyflow.packageId; version = $selected; architecture = 'x64'; sourceRevision = $descriptor.sourceRevision }
    [void](Test-LyFlowRuntimeDirectory $native $config $locked)
    Set-LyFlowDependencyFiles $RepositoryRoot $locked
    Write-Host "lyFlow 已显式更新并锁定为 $selected（客户端修订 $($locked.sourceRevision)）。"
} finally { Remove-LyFlowCacheDirectory $temporary $cacheRoot }
