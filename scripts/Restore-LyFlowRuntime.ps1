#Requires -Version 7.0
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '..'),
    [string]$RuntimeConfigPath,
    [string]$LockPath,
    [string]$CredentialFile,
    [string]$OutputFile
)
. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$RepositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$config = Get-LyFlowDependencyConfig $RepositoryRoot $RuntimeConfigPath
$locked = Get-LyFlowDependencyLock $RepositoryRoot $config $LockPath
$native = Restore-LyFlowDependency $RepositoryRoot $config $locked $CredentialFile
if ($OutputFile) { [IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile), $native) }
$native
