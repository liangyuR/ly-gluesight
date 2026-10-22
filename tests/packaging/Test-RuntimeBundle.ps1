#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../scripts/Runtime-Common.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../../src-tauri/target/packaging-test-$([guid]::NewGuid().ToString('N'))"))
$passed = 0
function Expect-Failure([string]$Name, [scriptblock]$Action, [string]$Message) {
    $failure = $null
    try { & $Action } catch { $failure = $_.Exception.Message }
    if (-not $failure -or $failure -notmatch $Message) { throw "$Name 未按预期失败：$failure" }
    $script:passed++
}
function Save-FixtureManifest($Manifest) {
    $Manifest | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $root 'runtime/manifest.json') -Encoding utf8NoBOM
}
try {
    [void][IO.Directory]::CreateDirectory((Join-Path $root 'runtime/lyflow'))
    [void][IO.Directory]::CreateDirectory((Join-Path $root 'runtime/mvs/Licenses'))
    $image = [byte[]]::new(128)
    $image[0] = 0x4d; $image[1] = 0x5a; $image[0x3c] = 64
    $image[64] = 0x50; $image[65] = 0x45; $image[68] = 0x64; $image[69] = 0x86
    [IO.File]::WriteAllBytes((Join-Path $root 'runtime/lyflow/lyflow_core.dll'), $image)
    'license' | Set-Content -LiteralPath (Join-Path $root 'runtime/mvs/Licenses/notice.txt')
    $config = Get-RuntimeConfiguration
    $inSeries = "$($config.lyflow.series).0"
    $otherSeries = "$([int]$config.lyflow.series.Split('.')[0] + 1).0.0"
    $manifest = @{ schemaVersion=2; bundleVersion = $config.bundleVersion; architecture = 'x64'; lyflow = @{version=$inSeries; abiVersion=$inSeries; localValidationOnly=$false; operators=$config.requiredOperators}; mvs = @{version=$config.mvsVersion}; files = @(Get-RuntimeFiles (Join-Path $root 'runtime')) }
    Save-FixtureManifest $manifest
    [void](Assert-RuntimeManifest (Join-Path $root 'runtime'))
    $passed++
    $manifest.bundleVersion = 0; Save-FixtureManifest $manifest
    Expect-Failure '旧包版本' { Assert-RuntimeManifest (Join-Path $root 'runtime') } '版本'
    $manifest.bundleVersion = $config.bundleVersion
    $manifest.mvs.version = '0.0.0.0'; Save-FixtureManifest $manifest
    Expect-Failure 'MVS版本错误' { Assert-RuntimeManifest (Join-Path $root 'runtime') } '版本'
    $manifest.mvs.version = $config.mvsVersion
    $manifest.architecture = 'x86'; Save-FixtureManifest $manifest
    Expect-Failure '包架构错误' { Assert-RuntimeManifest (Join-Path $root 'runtime') } '架构'
    $manifest.architecture = 'x64'
    foreach ($invalidAbi in @(15, '15', $otherSeries, "$inSeries ", '', $null)) {
        $manifest.lyflow.abiVersion = $invalidAbi; Save-FixtureManifest $manifest
        Expect-Failure "错误 ABI '$invalidAbi'" { Assert-RuntimeManifest (Join-Path $root 'runtime') } 'ABI'
    }
    $manifest.lyflow.abiVersion = $inSeries
    foreach ($invalidVersion in @(15, $otherSeries, "$inSeries ", '', $null)) {
        $manifest.lyflow.version = $invalidVersion; Save-FixtureManifest $manifest
        Expect-Failure "错误引擎版本 '$invalidVersion'" { Assert-RuntimeManifest (Join-Path $root 'runtime') } '引擎版本'
    }
    $manifest.lyflow.version = $inSeries
    Expect-Failure '旧客户端数字 ABI' { Assert-LyFlowVersions 15 $inSeries '应用客户端' } 'ABI'
    Expect-Failure '实际引擎与打包版本失配' { Assert-LyFlowVersions $inSeries "$($config.lyflow.series).1" '应用客户端' $inSeries } '不一致'
    foreach ($localOnly in @($true, 'false', $null)) {
        $manifest.lyflow.localValidationOnly = $localOnly; Save-FixtureManifest $manifest
        Expect-Failure '本机诊断候选' { Assert-RuntimeManifest (Join-Path $root 'runtime') } 'localValidationOnly'
    }
    $manifest.lyflow.localValidationOnly = $false; Save-FixtureManifest $manifest
    $image[68] = 0x4c; $image[69] = 0x01
    [IO.File]::WriteAllBytes((Join-Path $root 'runtime/lyflow/lyflow_core.dll'), $image)
    Expect-Failure 'x86 PE' { Assert-RuntimeManifest (Join-Path $root 'runtime') } 'x64'
    Expect-Failure '清理越界' { Reset-RuntimeStage (Join-Path $PSScriptRoot '../outside-stage') @() } '本项目'
    Expect-Failure '来源位于清理目录' { Reset-RuntimeStage $root @((Join-Path $root 'runtime')) } '来源'
    Expect-Failure '来源等于清理目录' { Reset-RuntimeStage $root @($root) } '来源'
    $source = Join-Path $root 'collect-source'
    [void][IO.Directory]::CreateDirectory((Join-Path $source 'ThirdParty'))
    [void][IO.Directory]::CreateDirectory((Join-Path $source 'recordings'))
    [void][IO.Directory]::CreateDirectory((Join-Path $source 'licenses/vcpkg/boost-filesystem'))
    [void][IO.Directory]::CreateDirectory((Join-Path $source 'licenses/visual-cpp'))
    $licenses = @('licenses/vcpkg/boost-filesystem/copyright', 'licenses/visual-cpp/1033-Redist.rtf')
    foreach ($name in (@('core.dll', 'ThirdParty/provider.cti', 'ThirdParty/settings.ini', 'debug.pdb', 'recordings/capture.txt', 'recipe.lyflow.json', 'source.cpp') + $licenses)) { 'fixture' | Set-Content -LiteralPath (Join-Path $source $name) }
    Copy-RuntimeTree $source (Join-Path $root 'collected-mvs')
    if (-not (Test-Path -LiteralPath (Join-Path $root 'collected-mvs/ThirdParty/provider.cti')) -or (Test-Path -LiteralPath (Join-Path $root 'collected-mvs/debug.pdb'))) { throw 'MVS递归收集规则错误。' }
    $passed++
    Copy-RuntimeTree $source (Join-Path $root 'collected-lyflow') -LyFlow
    if ((Test-Path -LiteralPath (Join-Path $root 'collected-lyflow/recordings/capture.txt')) -or (Test-Path -LiteralPath (Join-Path $root 'collected-lyflow/recipe.lyflow.json')) -or -not (Test-Path -LiteralPath (Join-Path $root 'collected-lyflow/ThirdParty/settings.ini'))) { throw 'lyFlow运行库与开发数据筛选规则错误。' }
    $passed++
    foreach ($license in $licenses) {
        if (-not (Test-Path -LiteralPath (Join-Path $root "collected-lyflow/$license") -PathType Leaf)) { throw "lyFlow许可证未完整保留：$license" }
    }
    foreach ($developmentFile in @('debug.pdb', 'source.cpp', 'recordings/capture.txt', 'recipe.lyflow.json')) {
        if (Test-Path -LiteralPath (Join-Path $root "collected-lyflow/$developmentFile")) { throw "lyFlow开发文件进入运行库：$developmentFile" }
    }
    $passed++
    if ($IsWindows) {
        & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -File (Join-Path $PSScriptRoot '../../scripts/Test-PackagedRuntime.ps1') -RuntimeRoot (Join-Path $root 'runtime') -ReportPath (Join-Path $root 'failure.json')
        if ($LASTEXITCODE -eq 0 -or (Get-Content -LiteralPath (Join-Path $root 'failure.json') -Raw | ConvertFrom-Json).success) { throw '独立进程没有返回失败报告。' }
        $passed++
    }
    Write-Host "运行库打包回归通过：$passed 项。"
} finally {
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
}
