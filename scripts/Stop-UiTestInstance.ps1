#Requires -Version 7.0
param(
    [Parameter(Mandatory)][ValidateSet('ui', 'p0', 'flow', 'e2e', 'soak')][string]$Profile,
    [Parameter(Mandatory)][string]$Instance
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'UiTest-Common.ps1')
$layout = Get-UiTestLayout -Profile $Profile
$owned = $Instance | ConvertFrom-Json
$process = Get-Process -Id $owned.pid -ErrorAction SilentlyContinue
if (!$process) { return }
if ($owned.identifier -ne $layout.Identifier -or $owned.executable -ne $layout.Executable -or
    $process.Path -ne $layout.Executable -or $process.StartTime.ToUniversalTime() -ne ([datetime]$owned.startedAt).ToUniversalTime()) {
    throw '测试进程身份已改变，未停止该进程。'
}
Stop-Process -InputObject $process
if (!$process.WaitForExit(15000)) { throw '隔离测试进程未能在 15 秒内退出。' }
