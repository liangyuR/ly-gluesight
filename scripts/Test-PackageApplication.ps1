#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$ApplicationPath,
    [string]$StageRoot = (Join-Path $PSScriptRoot '../src-tauri/target/package-runtime')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$ApplicationPath = [IO.Path]::GetFullPath($ApplicationPath)
$StageRoot = [IO.Path]::GetFullPath($StageRoot)
if (-not (Test-Path -LiteralPath $ApplicationPath -PathType Leaf)) { throw "找不到构建后的应用：$ApplicationPath" }
$releaseRoot = [IO.Path]::GetDirectoryName($ApplicationPath)
Assert-NoRuntimeLinks $releaseRoot -AncestorsOnly
$installedRuntime = Join-Path $releaseRoot 'runtime'
$sourceRuntime = Join-Path $StageRoot 'runtime'
if ([IO.Path]::GetFullPath($installedRuntime).Equals([IO.Path]::GetFullPath($sourceRuntime), [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝覆盖运行库来源目录。' }
if (Test-Path -LiteralPath $installedRuntime) {
    Assert-NoRuntimeLinks $installedRuntime
    Remove-Item -LiteralPath $installedRuntime -Recurse -Force
}
Copy-RuntimeTree $sourceRuntime $installedRuntime
foreach ($name in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
    $destination = Join-Path $releaseRoot $name
    if (Test-Path -LiteralPath $destination) { Assert-NoRuntimeLinks $destination }
    Copy-Item -LiteralPath (Join-Path $StageRoot "application-runtime/$name") -Destination $destination
}
$applicationReport = Join-Path $StageRoot 'application-runtime-validation.json'
$start = [Diagnostics.ProcessStartInfo]::new($ApplicationPath)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WorkingDirectory = $StageRoot
$start.ArgumentList.Add('--check-packaged-runtime')
$start.ArgumentList.Add($applicationReport)
$process = [Diagnostics.Process]::Start($start)
try {
    if (-not $process.WaitForExit(120000)) {
        $process.Kill($true)
        $process.WaitForExit()
        throw '应用运行库自检超过 120 秒；停止生成安装包。'
    }
    if ($process.ExitCode -ne 0) { throw "应用运行库自检失败（退出码 $($process.ExitCode)）；请查看 $applicationReport" }
} finally { $process.Dispose() }
if (-not (Test-Path -LiteralPath $applicationReport -PathType Leaf)) { throw '应用运行库自检未生成报告；停止生成安装包。' }
$result = Get-Content -LiteralPath $applicationReport -Raw | ConvertFrom-Json
if ($result.status -ne 'ok') { throw "应用运行库自检报告未通过；请查看 $applicationReport" }
# 定位冒烟：应用只报告状态（支持却冒烟失败时应用已退出非 0）；运行库不支持带外圈模板时按 locateSmoke 开关放行或失败
$locate = Get-RuntimeField (Get-RuntimeField $result 'lyflow') 'locate'
$locateStatus = Get-RuntimeField $locate 'status'
if ($locateStatus -ceq 'unsupported') {
    $message = "随包运行库不支持带外圈模板的特征定位：$(Get-RuntimeField $locate 'reason')；$(Get-RuntimeField $locate 'effect')。"
    if ((Get-LocateSmokePolicy (Get-RuntimeConfiguration)) -ceq 'required') { throw "$message packaging/runtime.json 的 lyflow.locateSmoke 为 required。" }
    Write-Warning "$message lyflow.locateSmoke 为 optional，只适用于没有特征定位的配方。"
} elseif ($locateStatus -cne 'ok') { throw "应用运行库自检报告缺少定位冒烟结果；请查看 $applicationReport" }
$stagedManifest = Get-Content -LiteralPath (Join-Path $sourceRuntime 'manifest.json') -Raw | ConvertFrom-Json
Assert-LyFlowVersions $result.lyflow.clientAbiVersion $result.lyflow.version '应用客户端与实际引擎' $stagedManifest.lyflow.version
