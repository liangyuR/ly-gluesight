#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../scripts/Vendor-Dependency.ps1')
$script:config = Get-RuntimeConfiguration
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../../src-tauri/target/vendor-dependency-test-$([guid]::NewGuid().ToString('N'))"))
$fixture = Join-Path $root 'fixture/runtime'
$cache = Join-Path $root 'cache'
$archive = Join-Path $root $script:config.asset
$destination = Join-Path $cache "$($script:config.tag)/$([IO.Path]::GetFileNameWithoutExtension($script:config.asset))"
$script:passed = 0
$script:downloads = 0
$script:downloadFailure = $false
$script:crtVersion = '14.44.35211.0'
function Get-RuntimeConfiguration { $script:config }
function Get-VendorCrtFileVersion([string]$Path) { $script:crtVersion }
function Expect-Failure([string]$Name, [scriptblock]$Action, [string]$Message) {
    $failure = $null
    try { & $Action | Out-Null } catch { $failure = $_.Exception.Message }
    if (-not $failure -or $failure -notmatch $Message) { throw "$Name 未按预期失败：$failure" }
    $script:passed++
}
function Save-VendorManifest($Manifest) {
    $Manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $fixture 'manifest.json') -Encoding utf8NoBOM
}
function New-VendorFixture {
    if (Test-Path -LiteralPath $fixture) { Remove-VendorCacheDirectory $fixture $root }
    [void][IO.Directory]::CreateDirectory((Join-Path $fixture 'mvs/Licenses'))
    [void][IO.Directory]::CreateDirectory((Join-Path $fixture 'crt'))
    'license' | Set-Content -LiteralPath (Join-Path $fixture 'mvs/Licenses/notice.txt') -Encoding utf8NoBOM
    $image = [byte[]]::new(128)
    $image[0] = 0x4d; $image[1] = 0x5a; $image[0x3c] = 64
    $image[64] = 0x50; $image[65] = 0x45; $image[68] = 0x64; $image[69] = 0x86
    foreach ($relative in $script:config.requiredMvsFiles) {
        $path = Join-Path $fixture "mvs/$relative"
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))
        if ([IO.Path]::GetExtension($relative) -in @('.dll', '.cti', '.ax', '.exe')) { [IO.File]::WriteAllBytes($path, $image) }
        else { 'configuration' | Set-Content -LiteralPath $path -Encoding utf8NoBOM }
    }
    $versions = [ordered]@{}
    foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        [IO.File]::WriteAllBytes((Join-Path $fixture "crt/$name"), $image)
        $versions[$name] = '14.44.35211.0'
    }
    $manifest = [pscustomobject]@{
        schemaVersion = 2; kind = 'vendor'; bundleVersion = $script:config.bundleVersion; architecture = 'x64'
        mvs = [pscustomobject]@{version = $script:config.mvsVersion}; crt = [pscustomobject]@{fileVersions = [pscustomobject]$versions}
        files = @(Get-RuntimeFiles $fixture)
    }
    Save-VendorManifest $manifest
    $manifest
}
function New-VendorArchive {
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force }
    [IO.Compression.ZipFile]::CreateFromDirectory((Join-Path $root 'fixture'), $archive)
}
function gh {
    $arguments = @($args)
    $script:downloads++
    if ($arguments[0] -cne 'release' -or $arguments[1] -cne 'download' -or $arguments[2] -cne $script:config.tag -or
        $arguments[[Array]::IndexOf($arguments, '--repo') + 1] -cne $script:config.repository -or
        $arguments[[Array]::IndexOf($arguments, '--pattern') + 1] -cne $script:config.asset) { throw '厂商恢复必须请求固定仓库/tag/asset。' }
    $directory = $arguments[[Array]::IndexOf($arguments, '--dir') + 1]
    if ($script:downloadFailure) {
        'partial download' | Set-Content -LiteralPath (Join-Path $directory $script:config.asset)
        $global:LASTEXITCODE = 1
        return
    }
    Copy-Item -LiteralPath $archive -Destination (Join-Path $directory $script:config.asset)
    $global:LASTEXITCODE = 0
}
try {
    New-VendorFixture | Out-Null
    [void](Assert-VendorRuntime $fixture); $script:passed++
    foreach ($field in @('schemaVersion', 'kind', 'bundleVersion', 'architecture')) {
        $manifest = New-VendorFixture
        $manifest.$field = switch ($field) { 'schemaVersion' {1}; 'kind' {'combined'}; 'bundleVersion' {0}; 'architecture' {'x86'} }
        Save-VendorManifest $manifest
        Expect-Failure "旧格式或错误 $field" { Assert-VendorRuntime $fixture } '旧合并'
    }
    $manifest = New-VendorFixture
    $manifest | Add-Member -NotePropertyName lyflow -NotePropertyValue ([pscustomobject]@{version = '1.1.0'})
    Save-VendorManifest $manifest
    Expect-Failure '混合包字段' { Assert-VendorRuntime $fixture } '旧合并'
    New-VendorFixture | Out-Null
    [void][IO.Directory]::CreateDirectory((Join-Path $fixture 'lyflow'))
    Expect-Failure '混合包目录' { Assert-VendorRuntime $fixture } '旧合并'
    $manifest = New-VendorFixture
    $manifest.mvs.version = '0.0.0.0'; Save-VendorManifest $manifest
    Expect-Failure 'MVS 版本' { Assert-VendorRuntime $fixture } '格式|版本'
    New-VendorFixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'mvs/MvCameraControl.dll')
    Expect-Failure '缺 MVS 组件' { Assert-VendorRuntime $fixture } '组件缺失'
    New-VendorFixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'crt/vcruntime140_1.dll')
    Expect-Failure '缺 CRT 组件' { Assert-VendorRuntime $fixture } '缺少 CRT'
    New-VendorFixture | Out-Null
    Remove-Item -LiteralPath (Join-Path $fixture 'mvs/Licenses/notice.txt')
    Expect-Failure '缺许可证' { Assert-VendorRuntime $fixture } '许可证'
    New-VendorFixture | Out-Null
    'extra bytes' | Add-Content -LiteralPath (Join-Path $fixture 'mvs/CommonParameters.ini')
    Expect-Failure '尺寸改变' { Assert-VendorRuntime $fixture } '尺寸'
    $manifest = New-VendorFixture
    $manifest.crt.fileVersions.'vcruntime140.dll' = '14.0.0.0'; Save-VendorManifest $manifest
    Expect-Failure 'CRT 实际版本不符' { Assert-VendorRuntime $fixture } 'CRT 文件版本'
    New-VendorFixture | Out-Null
    $script:crtVersion = ''
    Expect-Failure 'CRT 没有文件版本' { Assert-VendorRuntime $fixture } 'CRT 文件版本'
    $script:crtVersion = '14.44.35211.0'
    $manifest = New-VendorFixture
    $manifest.files[0].path = 'crt/../outside.dll'; Save-VendorManifest $manifest
    Expect-Failure '清单路径穿越' { Assert-VendorRuntime $fixture } '路径'
    New-VendorFixture | Out-Null
    New-VendorArchive
    $runtime = Restore-VendorRuntime $cache
    if (-not (Test-Path -LiteralPath $runtime) -or $script:downloads -ne 1) { throw '冷缓存必须下载并验证厂商包。' }; $script:passed++
    [void](Restore-VendorRuntime $cache)
    if ($script:downloads -ne 1) { throw '完整缓存必须复验后直接使用。' }; $script:passed++
    'corrupt size' | Add-Content -LiteralPath (Join-Path $runtime 'mvs/CommonParameters.ini')
    [void](Restore-VendorRuntime $cache)
    if ($script:downloads -ne 2) { throw '损坏缓存必须先删除再按固定源恢复。' }; $script:passed++
    Remove-Item -LiteralPath (Join-Path $runtime 'crt/vcruntime140.dll')
    $script:downloadFailure = $true
    Expect-Failure '下载失败不回退' { Restore-VendorRuntime $cache } '固定厂商依赖下载失败'
    if (Test-Path -LiteralPath $destination) { throw '损坏缓存删除后不能用失败下载建立版本缓存。' }; $script:passed++
    if (@(Get-ChildItem -LiteralPath $cache -Directory -Filter '.partial-*').Count) { throw '失败下载必须清理临时目录。' }; $script:passed++
    $script:downloadFailure = $false
    [void](Restore-VendorRuntime $cache); $script:passed++
    Expect-Failure '删除边界' { Remove-VendorCacheDirectory $root $cache } '以外'
    Expect-Failure '恢复边界' { Restore-VendorRuntime ([IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($root))) } '本项目'
    Remove-VendorCacheDirectory $destination $cache
    $linkTarget = Join-Path $root 'link-target'
    [void][IO.Directory]::CreateDirectory($linkTarget)
    'preserve' | Set-Content -LiteralPath (Join-Path $linkTarget 'sentinel.txt')
    [void](New-Item -ItemType Junction -Path $destination -Target $linkTarget)
    try {
        $before = $script:downloads
        Expect-Failure '链接缓存不能删除或恢复' { Restore-VendorRuntime $cache } '重定向'
        if ($script:downloads -ne $before -or -not (Test-Path -LiteralPath (Join-Path $linkTarget 'sentinel.txt'))) { throw '链接目标必须保留且不能发起下载。' }; $script:passed++
    } finally { Remove-Item -LiteralPath $destination -Force }
    $versionDirectory = [IO.Path]::GetDirectoryName($destination)
    Remove-Item -LiteralPath $versionDirectory -Force
    [void](New-Item -ItemType Junction -Path $versionDirectory -Target $linkTarget)
    try {
        $before = $script:downloads
        Expect-Failure '版本父目录链接不能写入' { Restore-VendorRuntime $cache } '重定向'
        if ($script:downloads -ne $before -or @(Get-ChildItem -LiteralPath $linkTarget).Count -ne 1) { throw '版本父链接目标不能写入目录或文件。' }; $script:passed++
    } finally { Remove-Item -LiteralPath $versionDirectory -Force }
    Write-Host "厂商依赖测试通过：$($script:passed) 项（包隔离、结构、尺寸、CRT 版本、固定源恢复与缓存边界；CRT 版本读取及 gh 使用 fixture）。"
} finally { Remove-VendorCacheDirectory $root ([IO.Path]::GetDirectoryName($root)) }
