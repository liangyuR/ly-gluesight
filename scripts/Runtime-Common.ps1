#Requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RuntimeConfiguration([string]$Path = (Join-Path $PSScriptRoot '../packaging/runtime.json')) {
    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    Assert-LyFlowRuntimeConfig $config
    $config
}

function Get-RuntimeField($Object, [string]$Name) {
    if ($null -eq $Object -or $Object -is [string] -or $Object -is [ValueType]) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($property) { $property.Value }
}

function Assert-LyFlowRuntimeConfig($Config) {
    Assert-NoRuntimeDigests $Config
    if ((Get-RuntimeField $Config 'schemaVersion') -ne 2 -or (Get-RuntimeField $Config 'architecture') -cne 'x64') {
        throw '拒绝旧运行依赖配置：packaging/runtime.json 必须使用 schema 2 和 x64 架构。'
    }
    $lyflow = Get-RuntimeField $Config 'lyflow'
    if ($lyflow -isnot [Management.Automation.PSCustomObject]) { throw 'packaging/runtime.json 缺少 lyflow 配置。' }
    $names = [string]::Join(',', @($lyflow.PSObject.Properties.Name | Sort-Object))
    if ($names -cne 'repository,series') { throw "lyFlow 配置只允许 repository 与 series 字段，拒绝旧 NuGet 或锁定字段：$names" }
    if ($lyflow.repository -isnot [string] -or $lyflow.repository -cnotmatch '^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$') { throw "lyFlow 仓库必须写成 owner/name：$($lyflow.repository)" }
    if ($lyflow.series -isnot [string] -or $lyflow.series -cnotmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw "lyFlow 系列必须写成 主.次：$($lyflow.series)" }
}

function Test-LyFlowSeriesVersion($Version, [string]$Series) {
    $Version -is [string] -and $Version -cmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$' -and
        $Version.Substring(0, $Version.LastIndexOf('.')) -ceq $Series
}

function Assert-LyFlowVersions($AbiVersion, $EngineVersion, [string]$Context, [string]$ExpectedEngineVersion) {
    $series = (Get-RuntimeConfiguration).lyflow.series
    if (-not (Test-LyFlowSeriesVersion $AbiVersion $series)) { throw "$Context 的 lyFlow ABI 版本 '$AbiVersion' 不属于 $series 系列。" }
    if (-not (Test-LyFlowSeriesVersion $EngineVersion $series)) { throw "$Context 的 lyFlow 引擎版本 '$EngineVersion' 不属于 $series 系列。" }
    if ($ExpectedEngineVersion -and $EngineVersion -cne $ExpectedEngineVersion) {
        throw "$Context 的 lyFlow 引擎版本 '$EngineVersion' 与所选版本 '$ExpectedEngineVersion' 不一致。"
    }
}

function Assert-LyFlowReleaseMetadata($Metadata, [string]$Context) {
    $localOnly = Get-RuntimeField $Metadata 'localValidationOnly'
    if ($localOnly -isnot [bool] -or $localOnly) {
        throw "$Context 必须显式声明 localValidationOnly=false；拒绝仅本机诊断的 lyFlow 候选。"
    }
}

function Assert-RuntimePlatform {
    if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
        throw '请使用 Windows x64 PowerShell 7。'
    }
}

function Assert-NoRuntimeLinks([string]$Path, [switch]$AncestorsOnly) {
    $item = Get-Item -LiteralPath $Path -Force
    $cursor = $item
    while ($null -ne $cursor) {
        if ($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "拒绝重定向路径：$($cursor.FullName)" }
        $cursor = if ($cursor -is [IO.DirectoryInfo]) { $cursor.Parent } else { $cursor.Directory }
    }
    if (-not $AncestorsOnly -and $item -is [IO.DirectoryInfo]) {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Recurse -Force) {
            if ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "拒绝重定向文件：$($child.FullName)" }
        }
    }
}

function Get-RuntimeFiles([string]$Root) {
    Assert-NoRuntimeLinks $Root
    @(Get-ChildItem -LiteralPath $Root -File -Recurse -Force | Sort-Object FullName | ForEach-Object {
        [pscustomobject]@{ path = [IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/'); bytes = $_.Length }
    })
}

function Assert-X64RuntimeImage([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5a4d) { throw "不是有效 PE 文件：$Path" }
        $stream.Position = 0x3c
        $offset = $reader.ReadUInt32()
        if ($offset -gt $stream.Length - 24) { throw "PE 头越界：$Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw "不是 Windows x64 运行库：$Path" }
    } finally { $reader.Dispose(); $stream.Dispose() }
}

function Assert-NoRuntimeDigests($Value) {
    if ($null -eq $Value -or $Value -is [string] -or $Value -is [ValueType]) { return }
    if ($Value -is [System.Collections.IEnumerable] -and $Value -isnot [pscustomobject]) {
        foreach ($entry in $Value) { Assert-NoRuntimeDigests $entry }
    } else {
        foreach ($property in $Value.PSObject.Properties) {
            if ($property.Name -match '(?i)hash|fingerprint|sha\d*|md5') { throw "拒绝带内容摘要字段的旧运行库记录：$($property.Name)" }
            Assert-NoRuntimeDigests $property.Value
        }
    }
}

function Reset-RuntimeStage([string]$StageRoot, [string[]]$Sources) {
    $projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $allowed = [IO.Path]::GetFullPath((Join-Path $projectRoot 'src-tauri/target')) + [IO.Path]::DirectorySeparatorChar
    $stage = [IO.Path]::GetFullPath($StageRoot)
    if (-not $stage.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase)) { throw '打包暂存目录必须位于本项目 src-tauri/target 下。' }
    $cursor = $stage
    while (-not (Test-Path -LiteralPath $cursor)) { $cursor = [IO.Path]::GetDirectoryName($cursor) }
    Assert-NoRuntimeLinks $cursor -AncestorsOnly
    foreach ($source in $Sources) {
        $sourcePath = [IO.Path]::GetFullPath($source).TrimEnd('\', '/')
        if ($sourcePath.Equals($stage, [StringComparison]::OrdinalIgnoreCase) -or $sourcePath.StartsWith($stage + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw '运行库来源不能位于会被重新生成的暂存目录。'
        }
        Assert-NoRuntimeLinks $sourcePath
    }
    if (Test-Path -LiteralPath $stage) {
        Assert-NoRuntimeLinks $stage
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
    [void][IO.Directory]::CreateDirectory($stage)
}

function Copy-RuntimeTree([string]$Source, [string]$Destination, [switch]$LyFlow) {
    Assert-NoRuntimeLinks $Source
    [void][IO.Directory]::CreateDirectory($Destination)
    foreach ($file in Get-ChildItem -LiteralPath $Source -Recurse -File -Force) {
        $relative = [IO.Path]::GetRelativePath($Source, $file.FullName)
        $license = $LyFlow -and $relative -match '(?i)^licenses?[\\/]'
        if (-not $license) {
            if ($LyFlow -and $file.Extension -notin @('.dll', '.cti', '.ini', '.manifest', '.txt', '.lic', '.xml', '.yaml', '.yml', '.json')) { continue }
            if ($file.Extension -in @('.pdb', '.ilk', '.exp', '.lib')) { continue }
            if ($LyFlow -and ($relative -match '(?i)(^|[\\/])(tests?|examples?|samples?|recipes?|history|recordings?|logs?|debug)([\\/]|$)' -or $file.BaseName -match '(?i)(^|[_-])(test|debug|example)([_-]|$)' -or $file.Name -like '*.lyflow.json')) { continue }
        }
        $target = Join-Path $Destination $relative
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))
        Copy-Item -LiteralPath $file.FullName -Destination $target
    }
}

function Assert-RuntimeManifest([string]$RuntimeRoot) {
    $config = Get-RuntimeConfiguration
    Assert-NoRuntimeLinks $RuntimeRoot
    $manifest = Get-Content -LiteralPath (Join-Path $RuntimeRoot 'manifest.json') -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $manifest
    if ($manifest.schemaVersion -ne 2 -or $manifest.bundleVersion -ne $config.bundleVersion -or $manifest.architecture -ne $config.architecture -or
        $manifest.mvs.version -ne $config.mvsVersion) { throw '运行库包版本、架构或 MVS 版本不符合固定配置。' }
    Assert-LyFlowVersions $manifest.lyflow.abiVersion $manifest.lyflow.version '依赖包清单'
    Assert-LyFlowReleaseMetadata $manifest.lyflow '依赖包清单'
    $actual = @(Get-RuntimeFiles $RuntimeRoot | Where-Object path -ne 'manifest.json')
    if ($actual.Count -ne @($manifest.files).Count) { throw '运行库文件清单数量不一致。' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $manifest.files) {
        if (-not $seen.Add([string]$file.path) -or $file.path -notmatch '^(lyflow|mvs|crt)/' -or $file.path.Contains('\') -or $file.path.Split('/') -contains '..') { throw "运行库文件清单路径无效：$($file.path)" }
        $match = @($actual | Where-Object path -CEQ $file.path)
        if ($match.Count -ne 1 -or $match[0].bytes -ne $file.bytes) { throw "运行库文件缺失或尺寸不符：$($file.path)" }
    }
    foreach ($file in Get-ChildItem -LiteralPath $RuntimeRoot -File -Recurse | Where-Object Extension -in @('.dll', '.cti', '.ax', '.exe')) { Assert-X64RuntimeImage $file.FullName }
    $manifest
}

function Assert-MvsRuntimeStructure([string]$MvsRoot) {
    $config = Get-RuntimeConfiguration
    foreach ($relative in $config.requiredMvsFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $MvsRoot $relative) -PathType Leaf)) { throw "MVS 必需组件缺失：$relative" }
    }
}

function Expand-RuntimeArchive([string]$Archive, [string]$Destination, [string]$EntryPrefix = 'runtime/') {
    $root = [IO.Path]::GetFullPath($Destination).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            if (-not $name.StartsWith($EntryPrefix, [StringComparison]::Ordinal) -or $name.StartsWith('/') -or $name.Contains(':') -or $name.Split('/') -contains '..' -or -not $seen.Add($name)) { throw "依赖 ZIP 条目路径无效：$name" }
            $target = [IO.Path]::GetFullPath((Join-Path $Destination $name))
            if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "依赖 ZIP 条目逃出暂存目录：$name" }
            $unixType = ($entry.ExternalAttributes -shr 16) -band 0xf000
            if ($unixType -eq 0xa000) { throw "拒绝 ZIP 符号链接：$name" }
        }
    } finally { $zip.Dispose() }
    Expand-Archive -LiteralPath $Archive -DestinationPath $Destination
}
