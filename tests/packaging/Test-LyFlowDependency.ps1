#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../scripts/LyFlow-Dependency.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../../src-tauri/target/lyflow-dependency-test-$([guid]::NewGuid().ToString('N'))"))
$fixture = Join-Path $root 'fixture/runtimes/win-x64/native'
$config = [pscustomobject]@{
    lyflow = [pscustomobject]@{ packageId = 'LyFlow.Runtime.win-x64'; source = 'https://nuget.pkg.github.com/liangyuR/index.json'; series = '1.1'; minimumVersion = '1.1.0' }
    requiredOperators = @('glue.taught_path', 'glue.bead_width')
}
$locked = [pscustomobject]@{ schemaVersion = 1; packageId = $config.lyflow.packageId; version = '1.1.0'; architecture = 'x64'; sourceRevision = ('a' * 40) }
$passed = 0
$script:downloads = 0
$script:interruptDownload = $false
$script:metadataReader = (Get-Command Invoke-LyFlowNuGetMetadata).ScriptBlock
$script:metadataRequests = [Collections.Generic.List[string]]::new()
function Expect-Failure([string]$Name, [scriptblock]$Action, [string]$Message) {
    $failure = $null
    try { & $Action | Out-Null } catch { $failure = $_.Exception.Message }
    if (-not $failure -or $failure -notmatch $Message) { throw "$Name 未按预期失败：$failure" }
    $script:passed++
}
function Save-Descriptor($Descriptor) {
    $Descriptor | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $fixture 'lyflow-runtime.json') -Encoding utf8NoBOM
}
function New-Fixture {
    if (Test-Path -LiteralPath $fixture) { Remove-LyFlowCacheDirectory $fixture $root }
    [void][IO.Directory]::CreateDirectory((Join-Path $fixture 'licenses'))
    'license' | Set-Content -LiteralPath (Join-Path $fixture 'licenses/notice.txt') -Encoding utf8NoBOM
    $image = [byte[]]::new(128)
    $image[0] = 0x4d; $image[1] = 0x5a; $image[0x3c] = 64
    $image[64] = 0x50; $image[65] = 0x45; $image[68] = 0x64; $image[69] = 0x86
    foreach ($name in @('lyflow_core.dll', 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) { [IO.File]::WriteAllBytes((Join-Path $fixture $name), $image) }
    $descriptor = [pscustomobject]@{
        schemaVersion = 1; architecture = 'x64'; engineVersion = '1.1.0'; abiVersion = '1.1.0'; sourceRevision = ('a' * 40)
        localValidationOnly = $false; operators = @($config.requiredOperators | ForEach-Object { [pscustomobject]@{ id = $_ } })
        files = @(@(Get-ChildItem -LiteralPath $fixture -File -Recurse | ForEach-Object { [pscustomobject]@{path = [IO.Path]::GetRelativePath($fixture, $_.FullName).Replace('\', '/')} }) + [pscustomobject]@{path = 'lyflow-runtime.json'})
    }
    Save-Descriptor $descriptor
    $script:dllMetadata = [pscustomobject]@{ version = '1.1.0'; abiVersion = '1.1.0'; sourceRevision = ('a' * 40); operators = $config.requiredOperators }
    $descriptor
}
function Invoke-LyFlowDllProbe([string]$RuntimeRoot) { $script:dllMetadata }
function Invoke-LyFlowNuGetMetadata([string]$Uri, [string]$Source, [string]$CredentialFile) {
    $script:metadataRequests.Add($Uri)
    if ($Uri -ceq $config.lyflow.source) { return [pscustomobject]@{resources = @([pscustomobject]@{'@type' = 'PackageBaseAddress/3.0.0'; '@id' = 'https://nuget.pkg.github.com/liangyuR/download'})} }
    if ($Uri -cne 'https://nuget.pkg.github.com/liangyuR/download/lyflow.runtime.win-x64/index.json') { throw '版本发现必须使用标准 PackageBaseAddress 包版本索引。' }
    [pscustomobject]@{versions = @('1.1.9', '1.1.10', '1.2.0', '2.0.0', '1.1.11-preview')}
}
function Invoke-LyFlowNuGet([string]$RepositoryRoot, [string]$Source, [string[]]$Arguments, [string]$CredentialFile) {
    $script:downloads++
    if ($Arguments -contains 'list') { throw '普通恢复不允许查询最新版本。' }
    if ($Arguments[[Array]::IndexOf($Arguments, '-Version') + 1] -cne $locked.version) { throw '普通恢复必须请求锁定的完整版本。' }
    $directory = $Arguments[[Array]::IndexOf($Arguments, '-OutputDirectory') + 1]
    $target = Join-Path $directory "$($config.lyflow.packageId)/runtimes/win-x64/native"
    [void][IO.Directory]::CreateDirectory($target)
    if ($script:interruptDownload) { 'partial' | Set-Content -LiteralPath (Join-Path $target 'partial'); throw '模拟下载中断' }
    foreach ($item in Get-ChildItem -LiteralPath $fixture -Force) { Copy-Item -LiteralPath $item.FullName -Destination $target -Recurse }
}
function Invoke-LyFlowCargoUpdate([string]$Revision) {
    if ($PWD.Path -cne (Join-Path $root 'src-tauri')) { throw 'Cargo 更新没有使用正确的工作目录。' }
    $path = Join-Path $root 'src-tauri/Cargo.lock'
    if ($script:updateFailure -eq 'cargo') {
        [IO.File]::WriteAllText($path, 'incomplete cargo lock')
        throw '模拟 Cargo 更新失败'
    }
    $text = $script:originalCargoLock.Replace(('a' * 40), $Revision).Replace('version = "1.1.0"', 'version = "1.1.1"')
    if ($script:updateFailure -eq 'plc') { $text = $text.Replace('#plc-revision', '#wrong-plc-revision') }
    if ($script:updateFailure -eq 'client') { $text = $text.Replace('version = "1.1.1"', 'version = "1.1.2"') }
    [IO.File]::WriteAllText($path, $text)
}
try {
    New-Fixture | Out-Null
    $discovered = @(Get-LyFlowAvailableVersions $root $config)
    if ($script:metadataRequests.Count -ne 2 -or (Resolve-LyFlowPackageVersion $discovered $config.lyflow) -cne '1.1.10') { throw '默认 latest 必须从标准 v3 版本索引中选择兼容系列补丁。' }; $passed++
    foreach ($uri in @('https://example.com/liangyuR/download/package/index.json', 'http://nuget.pkg.github.com/liangyuR/download/package/index.json', 'https://nuget.pkg.github.com/another-owner/download/package/index.json', 'https://user@nuget.pkg.github.com/liangyuR/download/package/index.json')) {
        Expect-Failure '元数据端点边界' { & $script:metadataReader $uri $config.lyflow.source } '相同 HTTPS'
    }
    if ((Resolve-LyFlowPackageVersion @('1.1.9', '1.1.10', '1.2.0', '1.1.11-preview') $config.lyflow) -cne '1.1.10') { throw '版本必须按数值选择最新稳定补丁。' }
    $passed++
    if ((Resolve-LyFlowPackageVersion @('1.1.10') $config.lyflow '1.1.0') -cne '1.1.0') { throw '指定版本应支持同系列回退。' }
    $passed++
    foreach ($version in @('1.2.0', '2.0.0', '1.0.99', '1.1.1-preview', '1.1.1+build', '01.1.0', '1.1', '1.1.0 ')) {
        Expect-Failure "拒绝版本 $version" { Resolve-LyFlowPackageVersion @() $config.lyflow $version } '系列|正式版本'
    }
    Expect-Failure '没有兼容版本' { Resolve-LyFlowPackageVersion @('1.2.0') $config.lyflow } '没有'
    [void](Test-LyFlowRuntimeDirectory $fixture $config $locked); $passed++
    foreach ($field in @('architecture', 'abiVersion', 'engineVersion', 'sourceRevision', 'localValidationOnly')) {
        $descriptor = New-Fixture
        $descriptor.$field = switch ($field) { 'architecture' {'x86'}; 'abiVersion' {'1.1.1'}; 'engineVersion' {'1.1.1'}; 'sourceRevision' {('b' * 40)}; 'localValidationOnly' {$true} }
        Save-Descriptor $descriptor
        Expect-Failure "错误描述 $field" { Test-LyFlowRuntimeDirectory $fixture $config $locked } '架构|版本|sourceRevision|localValidationOnly'
    }
    $descriptor = New-Fixture
    $descriptor | Add-Member -NotePropertyName hash -NotePropertyValue 'obsolete'; Save-Descriptor $descriptor
    Expect-Failure '旧摘要字段' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '摘要'
    New-Fixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'lyflow-runtime.json')
    Expect-Failure '缺失描述' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '不存在|exist|find'
    New-Fixture | Out-Null
    '{' | Set-Content -LiteralPath (Join-Path $fixture 'lyflow-runtime.json')
    Expect-Failure '损坏描述' { Test-LyFlowRuntimeDirectory $fixture $config $locked } 'JSON|Json'
    New-Fixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'lyflow_core.dll')
    Expect-Failure '缺 DLL' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '数量|缺失|DLL'
    foreach ($field in @('version', 'abiVersion', 'sourceRevision', 'operators')) {
        New-Fixture | Out-Null
        $script:dllMetadata.$field = switch ($field) { 'sourceRevision' {('b' * 40)}; 'operators' { @('glue.taught_path') }; default {'1.1.1'} }
        Expect-Failure "实际 DLL 错误 $field" { Test-LyFlowRuntimeDirectory $fixture $config $locked } '版本|sourceRevision|算子'
    }
    $descriptor = New-Fixture
    $descriptor.operators = @([pscustomobject]@{id = 'glue.taught_path'}); Save-Descriptor $descriptor
    Expect-Failure '描述缺算子' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '算子'
    $descriptor = New-Fixture
    $descriptor.files[0].path = '../outside.dll'; Save-Descriptor $descriptor
    Expect-Failure '描述路径穿越' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '路径'
    New-Fixture | Out-Null
    $pe = [IO.File]::ReadAllBytes((Join-Path $fixture 'lyflow_core.dll'))
    $pe[68] = 0x4c; $pe[69] = 0x01
    [IO.File]::WriteAllBytes((Join-Path $fixture 'lyflow_core.dll'), $pe)
    Expect-Failure 'x86 DLL' { Test-LyFlowRuntimeDirectory $fixture $config $locked } 'x64'
    New-Fixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'licenses/notice.txt')
    $descriptor = Get-Content -LiteralPath (Join-Path $fixture 'lyflow-runtime.json') -Raw | ConvertFrom-Json
    $descriptor.files = @($descriptor.files | Where-Object path -ne 'licenses/notice.txt'); Save-Descriptor $descriptor
    Expect-Failure '空许可证目录' { Test-LyFlowRuntimeDirectory $fixture $config $locked } '许可证'
    New-Fixture | Out-Null
    $native = Restore-LyFlowDependency $root $config $locked
    if (-not (Test-Path -LiteralPath $native) -or $script:downloads -ne 1) { throw '首次恢复必须下载已锁定包。' }; $passed++
    [void](Restore-LyFlowDependency $root $config $locked)
    if ($script:downloads -ne 1) { throw '完整缓存应重新验证后复用。' }; $passed++
    Remove-Item -LiteralPath (Join-Path $native 'lyflow_core.dll')
    [void](Restore-LyFlowDependency $root $config $locked)
    if ($script:downloads -ne 2) { throw '不完整缓存必须失效并重新下载固定版本。' }; $passed++
    $packageCache = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $native))
    Remove-LyFlowCacheDirectory $packageCache (Join-Path $root 'src-tauri/target/dependency-cache/nuget')
    $script:interruptDownload = $true
    Expect-Failure '下载中断' { Restore-LyFlowDependency $root $config $locked } '中断'
    if (Test-Path -LiteralPath $packageCache) { throw '中断下载不能进入完整版本缓存。' }; $passed++
    if (@(Get-ChildItem -LiteralPath (Join-Path $root 'src-tauri/target/dependency-cache/nuget') -Directory -Filter '.restore-*').Count) { throw '中断临时目录必须清理。' }; $passed++
    $script:interruptDownload = $false
    [void](Restore-LyFlowDependency $root $config $locked); $passed++
    [void][IO.Directory]::CreateDirectory((Join-Path $root 'packaging'))
    $updatePaths = @((Join-Path $root 'packaging/lyflow.lock.json'), (Join-Path $root 'src-tauri/Cargo.toml'), (Join-Path $root 'src-tauri/Cargo.lock'))
    $script:originalCargoLock = "[[package]]`nname = `"lyflow-client`"`nversion = `"1.1.0`"`nsource = `"git+ssh://git@github.com/liangyuR/lyFlow.git?rev=$('a' * 40)#$('a' * 40)`"`n`n[[package]]`nname = `"ly-plc`"`nversion = `"0.2.0`"`nsource = `"git+ssh://git@github.com/liangyuR/ly-plc.git#plc-revision`"`n"
    [IO.File]::WriteAllText($updatePaths[0], ($locked | ConvertTo-Json))
    [IO.File]::WriteAllText($updatePaths[1], "lyflow-client = { git = `"ssh://git@github.com/liangyuR/lyFlow.git`", rev = `"$('a' * 40)`" }`n")
    [IO.File]::WriteAllText($updatePaths[2], $script:originalCargoLock)
    $originalFiles = @($updatePaths | ForEach-Object { ,([IO.File]::ReadAllBytes($_)) })
    $nextLock = [pscustomobject]@{schemaVersion = 1; packageId = $locked.packageId; version = '1.1.1'; architecture = 'x64'; sourceRevision = ('b' * 40)}
    foreach ($failure in @('cargo', 'client', 'plc')) {
        $script:updateFailure = $failure
        Expect-Failure "更新失败回滚 $failure" { Set-LyFlowDependencyFiles $root $nextLock } '失败|配套|ly-plc'
        for ($index = 0; $index -lt $updatePaths.Count; $index++) {
            if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($updatePaths[$index])) -cne [Convert]::ToBase64String($originalFiles[$index])) {
                throw "更新失败必须逐字节恢复所有依赖文件：$($updatePaths[$index])"
            }
        }
        $passed++
    }
    $script:updateFailure = ''
    Set-LyFlowDependencyFiles $root $nextLock
    if ((Get-Content -LiteralPath $updatePaths[0] -Raw | ConvertFrom-Json).version -cne '1.1.1' -or
        [IO.File]::ReadAllText($updatePaths[1]) -notmatch ('b' * 40) -or [IO.File]::ReadAllText($updatePaths[2]) -notmatch ('b' * 40)) { throw '成功更新必须同时同步 NuGet、Cargo 声明和锁定记录。' }
    $passed++
    Write-Host "lyFlow 依赖测试通过：$passed 项（版本、清单、DLL 元数据、缓存与中断恢复；DLL 探针及网络使用 fixture）。"
} finally { Remove-LyFlowCacheDirectory $root ([IO.Path]::GetDirectoryName($root)) }
