#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../scripts/LyFlow-Dependency.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../../src-tauri/target/lyflow-dependency-test-$([guid]::NewGuid().ToString('N'))"))
$repository = 'liangyuR/lyFlow'
$required = @('glue.taught_path', 'glue.bead_width')
$config = [pscustomobject]@{
    schemaVersion = 2; architecture = 'x64'
    lyflow = [pscustomobject]@{ repository = $repository; series = '1.1' }
    requiredOperators = $required
}
$passed = 0
$script:releases = @()
$script:releaseError = $null
$script:downloadError = $null
$script:downloads = 0
$script:assets = @{}
$script:dllMetadata = $null
$script:cargoMode = 'ok'

function Expect-Failure([string]$Name, [scriptblock]$Action, [string]$Message) {
    $failure = $null
    try { & $Action | Out-Null } catch { $failure = $_.Exception.Message }
    if (-not $failure -or $failure -notmatch $Message) { throw "$Name 未按预期失败：$failure" }
}
function Assert-True([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Invoke-Case([string]$Name, [scriptblock]$Body) {
    try { . $Body } catch { throw "[$Name] $($_.Exception.Message)" }
    $script:passed++
}

function New-RuntimeFixture([string]$Directory, [string]$Version, [scriptblock]$Mutate) {
    if (Test-Path -LiteralPath $Directory) { Remove-Item -LiteralPath $Directory -Recurse -Force }
    [void][IO.Directory]::CreateDirectory((Join-Path $Directory 'licenses'))
    'license' | Set-Content -LiteralPath (Join-Path $Directory 'licenses/notice.txt') -Encoding utf8NoBOM
    $image = [byte[]]::new(128)
    $image[0] = 0x4d; $image[1] = 0x5a; $image[0x3c] = 64
    $image[64] = 0x50; $image[65] = 0x45; $image[68] = 0x64; $image[69] = 0x86
    foreach ($name in @('lyflow_core.dll', 'msvcp140.dll', 'vcruntime140.dll')) { [IO.File]::WriteAllBytes((Join-Path $Directory $name), $image) }
    $descriptor = [pscustomobject]@{
        schemaVersion = 1; engineVersion = $Version; abiVersion = '1.1.0'; architecture = 'x64'; localValidationOnly = $false
        operators = @($required + 'io.load_image' | ForEach-Object { [pscustomobject]@{ id = $_ } })
        files = @(@('licenses/notice.txt', 'lyflow_core.dll', 'msvcp140.dll', 'vcruntime140.dll', 'lyflow-runtime.json') | ForEach-Object { [pscustomobject]@{ path = $_ } })
    }
    if ($Mutate) { & $Mutate $descriptor $Directory }
    $descriptor | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Directory 'lyflow-runtime.json') -Encoding utf8NoBOM
}

function New-ReleaseAsset([string]$Version, [scriptblock]$Mutate) {
    $source = Join-Path $root "asset-source/$Version"
    New-RuntimeFixture $source $Version $Mutate
    $archive = Join-Path $root "assets/lyflow-runtime-win-x64-v$Version.zip"
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($archive))
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive }
    [IO.Compression.ZipFile]::CreateFromDirectory($source, $archive)
    $script:assets[$Version] = $archive
}

function New-Release([string]$Tag, [switch]$Draft, [switch]$Prerelease, [switch]$NoAsset) {
    $version = $Tag -replace '^v', ''
    $assets = if ($NoAsset) { @([pscustomobject]@{ name = "lyflow-sdk-win-x64-$Tag.zip"; browser_download_url = "https://github.com/$repository/releases/download/$Tag/lyflow-sdk-win-x64-$Tag.zip" }) }
        else { @([pscustomobject]@{ name = "lyflow-runtime-win-x64-v$version.zip"; browser_download_url = "https://github.com/$repository/releases/download/$Tag/lyflow-runtime-win-x64-v$version.zip" }) }
    [pscustomobject]@{ tag_name = $Tag; draft = [bool]$Draft; prerelease = [bool]$Prerelease; assets = $assets }
}

function Get-LyFlowReleases([string]$Repository) {
    if ($script:releaseError) { throw $script:releaseError }
    $script:releases
}
function Save-LyFlowReleaseAsset($Release, [string]$Destination) {
    $script:downloads++
    if ($script:downloadError) { throw $script:downloadError }
    Copy-Item -LiteralPath $script:assets[$Release.version] -Destination $Destination
}
function Invoke-LyFlowDllProbe([string]$RuntimeRoot) {
    if ($script:dllMetadata) { return $script:dllMetadata }
    $descriptor = Get-Content -LiteralPath (Join-Path $RuntimeRoot 'lyflow-runtime.json') -Raw | ConvertFrom-Json
    [pscustomobject]@{ version = $descriptor.engineVersion; abiVersion = $descriptor.abiVersion; operators = @($descriptor.operators.id) }
}
function Invoke-LyFlowCargoUpdate {
    $path = Join-Path $PWD.Path 'Cargo.lock'
    if ($script:cargoMode -eq 'fail') { [IO.File]::WriteAllText($path, 'broken'); throw '模拟 cargo update 失败' }
    $tag = if ($script:cargoMode -eq 'stale') { 'v1.1.0' } else { 'v1.2.3' }
    [IO.File]::WriteAllText($path, "[[package]]`nname = `"lyflow-client`"`nversion = `"1.2.3`"`nsource = `"git+https://github.com/$repository.git?tag=$tag#0`"`n")
}

$cacheRoot = Get-LyFlowCacheRoot $root
$fixture = Join-Path $root 'fixture'
$cargoPath = Join-Path $root 'Cargo.toml'
$url = "https://github.com/$repository.git"
try {
    [void][IO.Directory]::CreateDirectory($root)

    Invoke-Case '混合 Release 列表 → 选系列内带运行库资产的最高正式补丁 1.1.10' {
        $mixed = @(
            (New-Release 'v1.1.9'), (New-Release 'v1.1.10'), (New-Release 'v1.1.11' -Draft), (New-Release 'v1.1.12' -Prerelease),
            (New-Release 'v1.1.13' -NoAsset), (New-Release 'v1.2.0'), (New-Release 'v2.0.0'), (New-Release 'v1.0.99'),
            (New-Release '1.1.14'), (New-Release 'v1.1.15-rc.1'), (New-Release 'v01.1.16')
        )
        Assert-True ((Select-LyFlowRelease $mixed $config).version -ceq '1.1.10') '系列最新正式 Release 选择错误。'
    }

    Invoke-Case '系列内无可用 Release → 失败，不跨系列' {
        Expect-Failure '系列无 Release' { Select-LyFlowRelease @((New-Release 'v1.2.0'), (New-Release 'v1.1.1' -Prerelease)) $config } '没有'
    }

    Invoke-Case 'lyflow 段含旧 NuGet 字段或系列格式错误 → 拒绝' {
        Assert-LyFlowRuntimeConfig $config
        foreach ($case in @(
            @{ packageId = 'LyFlow.Runtime.win-x64' }, @{ minimumVersion = '1.1.0' }, @{ series = '1' }, @{ series = '1.1.0' }, @{ series = '01.1' }, @{ repository = 'lyFlow' }
        )) {
            $broken = $config | ConvertTo-Json -Depth 5 | ConvertFrom-Json
            foreach ($key in $case.Keys) { $broken.lyflow | Add-Member -NotePropertyName $key -NotePropertyValue $case[$key] -Force }
            Expect-Failure "配置 $($case.Keys)" { Assert-LyFlowRuntimeConfig $broken } 'lyFlow'
        }
    }

    Invoke-Case 'lyflow-client 用 rev/branch/系列外 tag/多条/错地址 → 拒绝；系列内 tag → 接受' {
        [IO.File]::WriteAllText($cargoPath, "[dependencies]`nlyflow-client = { git = `"$url`", tag = `"v1.1.0`" }`n")
        Assert-True ((Get-LyFlowCargoTag $cargoPath $config) -ceq 'v1.1.0') '合法 tag 应被接受。'
        foreach ($line in @(
            "lyflow-client = { git = `"$url`", rev = `"893a0f3`" }",
            "lyflow-client = { git = `"$url`", branch = `"main`" }",
            "lyflow-client = { git = `"$url`", tag = `"v1.2.0`" }",
            "lyflow-client = { git = `"$url`", tag = `"1.1.0`" }",
            "lyflow-client = { git = `"$url`", tag = `"v1.1.0`", rev = `"893a0f3`" }",
            "lyflow-client = { git = `"https://github.com/other/lyFlow.git`", tag = `"v1.1.0`" }",
            "lyflow-client = { git = `"$url`", tag = `"v1.1.0`" }`nlyflow-client = { git = `"$url`", tag = `"v1.1.1`" }",
            'serde = "1"'
        )) {
            [IO.File]::WriteAllText($cargoPath, "[dependencies]`n$line`n")
            Expect-Failure "Cargo 依赖 $line" { Get-LyFlowCargoTag $cargoPath $config } 'lyflow-client|Cargo'
        }
    }

    Invoke-Case '完整运行库 → 校验通过' {
        New-RuntimeFixture $fixture '1.1.0'
        [void](Test-LyFlowRuntimeDirectory $fixture $config '1.1.0')
    }

    Invoke-Case '描述版本/架构/localValidationOnly/文件/算子任一不符 → 拒绝' {
        $script:dllMetadata = [pscustomobject]@{ version = '1.1.0'; abiVersion = '1.1.0'; operators = $required }
        foreach ($case in @(
            @{ name = '引擎版本不是所选 Release'; mutate = { param($d) $d.engineVersion = '1.1.1' } },
            @{ name = 'ABI 不属于系列'; mutate = { param($d) $d.abiVersion = '1.2.0' } },
            @{ name = '架构不是 x64'; mutate = { param($d) $d.architecture = 'x86' } },
            @{ name = '本机诊断候选'; mutate = { param($d) $d.localValidationOnly = $true } },
            @{ name = '缺少 localValidationOnly'; mutate = { param($d) $d.PSObject.Properties.Remove('localValidationOnly') } },
            @{ name = '清单文件缺失'; mutate = { param($d, $dir) Remove-Item -LiteralPath (Join-Path $dir 'msvcp140.dll') } },
            @{ name = '清单路径穿越'; mutate = { param($d, $dir) $d.files[0].path = '../outside.dll'; 'outside' | Set-Content -LiteralPath (Join-Path $dir '../outside.dll') } },
            @{ name = '缺必需算子'; mutate = { param($d) $d.operators = @([pscustomobject]@{ id = 'glue.taught_path' }) } }
        )) {
            New-RuntimeFixture $fixture '1.1.0' $case.mutate
            Expect-Failure $case.name { Test-LyFlowRuntimeDirectory $fixture $config '1.1.0' } '运行库'
        }
        $script:dllMetadata = $null
    }

    Invoke-Case '实际 DLL 版本与所选 Release 不同 → 拒绝' {
        New-RuntimeFixture $fixture '1.1.0'
        $script:dllMetadata = [pscustomobject]@{ version = '1.1.1'; abiVersion = '1.1.0'; operators = $required }
        Expect-Failure '实际 DLL 版本不符' { Test-LyFlowRuntimeDirectory $fixture $config '1.1.0' } '实际 DLL'
        $script:dllMetadata = $null
    }

    Invoke-Case '首次恢复 → 下载并写缓存；再次恢复 → 命中缓存不下载' {
        New-ReleaseAsset '1.1.1'
        $script:releases = @((New-Release 'v1.1.0'), (New-Release 'v1.1.1'))
        $first = Restore-LyFlowDependency $root $config
        $second = Restore-LyFlowDependency $root $config
        Assert-True ($first.version -ceq '1.1.1' -and $first.source -ceq 'download' -and $second.source -ceq 'cache' -and $script:downloads -eq 1) '缓存命中时不应重复下载。'
        Assert-True (Test-Path -LiteralPath (Join-Path $cacheRoot '1.1.1/lyflow_core.dll') -PathType Leaf) '运行库应解压到 dependency-cache/lyflow/<版本>/ 根目录。'
    }

    Invoke-Case '缓存缺文件 → 删除并重新下载' {
        Remove-Item -LiteralPath (Join-Path $cacheRoot '1.1.1/lyflow_core.dll')
        $repaired = Restore-LyFlowDependency $root $config
        Assert-True ($repaired.source -ceq 'download' -and $script:downloads -eq 2) '损坏缓存必须重新下载。'
    }

    Invoke-Case '最新 Release 校验失败 → 报错且不写缓存、不留临时目录' {
        New-ReleaseAsset '1.1.2' { param($d) $d.operators = @() }
        $script:releases = @((New-Release 'v1.1.1'), (New-Release 'v1.1.2'))
        Expect-Failure '最新 Release 校验失败' { Restore-LyFlowDependency $root $config } '算子'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $cacheRoot '1.1.2'))) '校验失败不得写入缓存。'
        Assert-True (-not @(Get-ChildItem -LiteralPath $cacheRoot -Directory -Force -Filter '.download-*').Count) '临时下载目录必须清理。'
    }

    Invoke-Case 'API 或下载失败 → 退回系列内最高的有效缓存' {
        [void][IO.Directory]::CreateDirectory((Join-Path $cacheRoot '1.1.5'))
        $script:releases = @((New-Release 'v1.1.6'))
        foreach ($mode in @('list', 'download')) {
            $script:releaseError = if ($mode -eq 'list') { '模拟 API 失败' } else { $null }
            $script:downloadError = if ($mode -eq 'download') { '模拟下载失败' } else { $null }
            $fallback = Restore-LyFlowDependency $root $config 3>$null
            Assert-True ($fallback.version -ceq '1.1.1' -and $fallback.source -ceq 'cache-fallback') "$mode 失败时应退回缓存 1.1.1，实际 $($fallback.version)。"
        }
    }

    Invoke-Case '升级时 API 失败 → 报错，不退回缓存' {
        $script:releaseError = '模拟 API 失败'; $script:downloadError = $null
        Expect-Failure '升级不退回缓存' { Restore-LyFlowDependency $root $config -NoFallback } 'API'
    }

    Invoke-Case 'API 失败且无有效缓存 → 恢复失败' {
        Expect-Failure '无缓存' { Restore-LyFlowDependency (Join-Path $root 'empty-repository') $config 3>$null } '没有'
        $script:releaseError = $null
    }

    $repo = Join-Path $root 'update-repository'
    [void][IO.Directory]::CreateDirectory((Join-Path $repo 'packaging'))
    [void][IO.Directory]::CreateDirectory((Join-Path $repo 'src-tauri'))
    $runtimeText = "{`n  `"schemaVersion`": 2,`n  `"architecture`": `"x64`",`n  `"lyflow`": {`n    `"repository`": `"$repository`",`n    `"series`": `"1.1`"`n  },`n  `"requiredOperators`": [`"glue.taught_path`", `"glue.bead_width`"]`n}`n"
    $cargoText = "[dependencies]`nserde = `"1`"`nlyflow-client = { git = `"$url`", tag = `"v1.1.0`" }`n"
    $lockText = "[[package]]`nname = `"lyflow-client`"`nversion = `"1.1.0`"`nsource = `"git+$url`?tag=v1.1.0#0`"`n"
    $updatePaths = @((Join-Path $repo 'packaging/runtime.json'), (Join-Path $repo 'src-tauri/Cargo.toml'), (Join-Path $repo 'src-tauri/Cargo.lock'))
    $originals = @($runtimeText, $cargoText, $lockText)

    Invoke-Case 'cargo update 失败或 Cargo.lock 未切换 → 三个依赖文件逐字节回滚' {
        foreach ($mode in @('fail', 'stale')) {
            for ($index = 0; $index -lt 3; $index++) { [IO.File]::WriteAllText($updatePaths[$index], $originals[$index]) }
            $script:cargoMode = $mode
            Expect-Failure "升级回滚 $mode" { Set-LyFlowDependencyFiles $repo '1.2' 'v1.2.3' } 'cargo|Cargo.lock'
            for ($index = 0; $index -lt 3; $index++) {
                Assert-True ([IO.File]::ReadAllText($updatePaths[$index]) -ceq $originals[$index]) "升级失败未回滚：$($updatePaths[$index])"
            }
        }
    }

    Invoke-Case '升级成功 → 只改 series 与 tag' {
        $script:cargoMode = 'ok'
        Set-LyFlowDependencyFiles $repo '1.2' 'v1.2.3'
        $updatedConfig = Get-LyFlowDependencyConfig $repo
        Assert-True ($updatedConfig.lyflow.series -ceq '1.2' -and (Get-LyFlowCargoTag $updatePaths[1] $updatedConfig) -ceq 'v1.2.3') '升级后系列与 tag 未同步。'
        Assert-True ([IO.File]::ReadAllText($updatePaths[0]) -ceq $runtimeText.Replace('"series": "1.1"', '"series": "1.2"')) 'runtime.json 只应改动 series。'
    }

    Write-Host "lyFlow 依赖测试通过：$passed 项（Release 选择、配置与 Cargo 锁定、运行库校验、缓存与网络回退、升级回滚；网络与 DLL 探针使用本地夹具）。"
} finally {
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
