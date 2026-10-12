#Requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RuntimeConfiguration {
    $config = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../packaging/runtime.json') -Raw | ConvertFrom-Json
    if ($config.schemaVersion -ne 2) { throw '拒绝旧运行依赖配置，请重建为拆分依赖格式。' }
    $locked = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../packaging/lyflow.lock.json') -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $locked
    if ($locked.schemaVersion -ne 1 -or $locked.packageId -cne $config.lyflow.packageId -or
        $locked.architecture -cne $config.architecture -or $locked.version -cnotmatch ('^' + [regex]::Escape($config.lyflow.series) + '\.\d+$') -or
        [version]$locked.version -lt [version]$config.lyflow.minimumVersion) { throw 'lyFlow 锁定记录不符合兼容系列、最低版本或包身份配置。' }
    Assert-LyFlowSourceRevision $locked.sourceRevision $locked.sourceRevision 'lyFlow 锁定记录'
    $config | Add-Member -NotePropertyName lyflowAbiVersion -NotePropertyValue $locked.version
    $config | Add-Member -NotePropertyName lyflowSourceRevision -NotePropertyValue $locked.sourceRevision
    $config
}

function Assert-LyFlowVersions($AbiVersion, $EngineVersion, [string]$Context) {
    $expected = (Get-RuntimeConfiguration).lyflowAbiVersion
    if ($AbiVersion -isnot [string] -or $EngineVersion -isnot [string] -or
        $AbiVersion -cne $expected -or $EngineVersion -cne $expected) {
        throw "$Context 的 lyFlow ABI 和引擎版本必须严格等于字符串 '$expected'；实际 ABI='$AbiVersion'，引擎='$EngineVersion'。不接受数字 ABI 或旧产物。"
    }
}

function Assert-LyFlowSourceRevision($Actual, $Expected, [string]$Context) {
    if ($Actual -isnot [string] -or $Expected -isnot [string] -or
        $Actual -cnotmatch '^[0-9a-f]{40}$' -or $Expected -cnotmatch '^[0-9a-f]{40}$' -or $Actual -cne $Expected) {
        throw "$Context 的 lyFlow sourceRevision 必须是与正式产物严格一致的 40 位 Git 修订；实际='$Actual'，期望='$Expected'。"
    }
}

function Assert-LyFlowReleaseMetadata($Metadata, [string]$Context) {
    Assert-LyFlowSourceRevision $Metadata.sourceRevision $Metadata.sourceRevision $Context
    if ($Metadata.localValidationOnly -isnot [bool] -or $Metadata.localValidationOnly) {
        throw "$Context 必须显式声明 localValidationOnly=false；拒绝 dirty 或仅本机诊断的 lyFlow 候选。"
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
    Assert-LyFlowSourceRevision $manifest.lyflow.sourceRevision $config.lyflowSourceRevision '依赖包与锁定记录'
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

function Expand-RuntimeArchive([string]$Archive, [string]$Destination) {
    $root = [IO.Path]::GetFullPath($Destination).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            if ($name -notmatch '^runtime/' -or $name.Contains(':') -or $name.Split('/') -contains '..' -or -not $seen.Add($name)) { throw "依赖 ZIP 条目路径无效：$name" }
            $target = [IO.Path]::GetFullPath((Join-Path $Destination $name))
            if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "依赖 ZIP 条目逃出暂存目录：$name" }
            $unixType = ($entry.ExternalAttributes -shr 16) -band 0xf000
            if ($unixType -eq 0xa000) { throw "拒绝 ZIP 符号链接：$name" }
        }
    } finally { $zip.Dispose() }
    Expand-Archive -LiteralPath $Archive -DestinationPath $Destination
}
