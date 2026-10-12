#Requires -Version 7.0
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')

function Get-VendorCrtFileVersion([string]$Path) {
    [Diagnostics.FileVersionInfo]::GetVersionInfo($Path).FileVersion
}

function Assert-VendorRuntime([string]$RuntimeRoot) {
    $config = Get-RuntimeConfiguration
    Assert-NoRuntimeLinks $RuntimeRoot
    $manifest = Get-Content -LiteralPath (Join-Path $RuntimeRoot 'manifest.json') -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $manifest
    if ($manifest.schemaVersion -ne 2 -or $manifest.kind -cne 'vendor' -or
        $manifest.bundleVersion -ne $config.bundleVersion -or $manifest.architecture -cne 'x64' -or
        $manifest.mvs.version -cne $config.mvsVersion -or $manifest.PSObject.Properties.Name -contains 'lyflow' -or
        (Test-Path -LiteralPath (Join-Path $RuntimeRoot 'lyflow'))) { throw '厂商依赖包格式、版本或架构不符；拒绝旧合并依赖包。' }
    Assert-MvsRuntimeStructure (Join-Path $RuntimeRoot 'mvs')
    if (-not @(Get-ChildItem -LiteralPath (Join-Path $RuntimeRoot 'mvs/Licenses') -File -Recurse).Count) { throw '厂商依赖包缺少 MVS 许可证。' }
    foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        $path = Join-Path $RuntimeRoot "crt/$name"
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "厂商依赖包缺少 CRT：$name" }
        $version = Get-VendorCrtFileVersion $path
        if ([string]::IsNullOrWhiteSpace($version) -or $version -cne $manifest.crt.fileVersions.$name) { throw "CRT 文件版本与清单不符：$name" }
    }
    $actual = @(Get-RuntimeFiles $RuntimeRoot | Where-Object path -ne 'manifest.json')
    if ($actual.Count -ne @($manifest.files).Count) { throw '厂商依赖包文件数量不符。' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $manifest.files) {
        if (-not $seen.Add($file.path) -or $file.path -notmatch '^(mvs|crt)/' -or $file.path.Contains('\') -or
            $file.path.Contains(':') -or $file.path.Split('/') -contains '..') { throw "厂商依赖路径无效：$($file.path)" }
        if ($file.path -match '^crt/(?!licenses?/)' -and $file.path -notmatch '^crt/(concrt140|msvcp140[^/]*|vccorlib140|vcomp140|vcruntime140[^/]*)\.dll$') { throw "CRT 目录混入非 CRT 文件：$($file.path)" }
        $match = @($actual | Where-Object path -CEQ $file.path)
        if ($match.Count -ne 1 -or $match[0].bytes -ne $file.bytes) { throw "厂商依赖文件缺失或尺寸不符：$($file.path)" }
    }
    foreach ($file in Get-ChildItem -LiteralPath $RuntimeRoot -File -Recurse | Where-Object Extension -in @('.dll', '.cti', '.ax', '.exe')) { Assert-X64RuntimeImage $file.FullName }
    return $manifest
}

function Remove-VendorCacheDirectory([string]$Path, [string]$CacheRoot) {
    $absolute = [IO.Path]::GetFullPath($Path)
    $prefix = [IO.Path]::GetFullPath($CacheRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $absolute.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝删除厂商缓存目录以外的路径。' }
    if (Test-Path -LiteralPath $absolute) { Assert-NoRuntimeLinks $absolute; Remove-Item -LiteralPath $absolute -Recurse -Force }
}

function Restore-VendorRuntime([string]$CacheRoot = (Join-Path $PSScriptRoot '../src-tauri/target/dependency-cache/vendor')) {
    $config = Get-RuntimeConfiguration
    if ($config.repository -cne 'liangyuR/ly-gluesight' -or $config.tag -cnotmatch '^deps-win-x64-v\d+$' -or
        $config.asset -cnotmatch '^gluesight-vendor-runtime-win-x64-v\d+\.zip$') { throw '厂商依赖必须来自显式版本化的本仓库 Release。' }
    $cacheRoot = [IO.Path]::GetFullPath($CacheRoot)
    $allowed = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../src-tauri/target')).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $cacheRoot.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase)) { throw '厂商缓存目录必须位于本项目 src-tauri/target 下。' }
    [void][IO.Directory]::CreateDirectory($cacheRoot)
    Assert-NoRuntimeLinks $cacheRoot
    $destination = Join-Path $cacheRoot "$($config.tag)/$([IO.Path]::GetFileNameWithoutExtension($config.asset))"
    $versionDirectory = [IO.Path]::GetDirectoryName($destination)
    if (Test-Path -LiteralPath $versionDirectory) { Assert-NoRuntimeLinks $versionDirectory }
    if (Test-Path -LiteralPath $destination) {
        Assert-NoRuntimeLinks $destination
        try {
            [void](Assert-VendorRuntime (Join-Path $destination 'runtime'))
            Write-Host "厂商依赖缓存命中：$($config.tag)"
            return (Join-Path $destination 'runtime')
        } catch {
            Write-Host "厂商依赖缓存损坏或不完整，清理后重新恢复固定 $($config.tag)。"
            Remove-VendorCacheDirectory $destination $cacheRoot
        }
    }
    $partial = Join-Path $cacheRoot ('.partial-' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($partial)
    try {
        & gh release download $config.tag --repo $config.repository --pattern $config.asset --dir $partial
        if ($LASTEXITCODE -ne 0) { throw '固定厂商依赖下载失败；不回退到其他版本。' }
        $expanded = Join-Path $partial 'expanded'
        Expand-RuntimeArchive (Join-Path $partial $config.asset) $expanded
        [void](Assert-VendorRuntime (Join-Path $expanded 'runtime'))
        [void][IO.Directory]::CreateDirectory($versionDirectory)
        Assert-NoRuntimeLinks $versionDirectory
        if (Test-Path -LiteralPath $destination) { throw '厂商依赖缓存被并发写入，请重试。' }
        Move-Item -LiteralPath $expanded -Destination $destination
        Write-Host "厂商依赖已恢复：$($config.tag)"
        return (Join-Path $destination 'runtime')
    } finally {
        Remove-VendorCacheDirectory $partial $cacheRoot
    }
}
