#Requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$script:LyFlowDependencyScriptPath = $PSCommandPath

function Get-LyFlowDependencyConfig([string]$RepositoryRoot, [string]$RuntimeConfigPath) {
    if (-not $RuntimeConfigPath) { $RuntimeConfigPath = Join-Path $RepositoryRoot 'packaging/runtime.json' }
    Get-RuntimeConfiguration $RuntimeConfigPath
}

function Get-LyFlowGitUrl($Config) { "https://github.com/$($Config.lyflow.repository).git" }

function Get-LyFlowCargoTag([string]$CargoTomlPath, $Config) {
    $text = [IO.File]::ReadAllText($CargoTomlPath)
    $entries = @([regex]::Matches($text, '(?m)^[ \t]*lyflow-client[ \t]*=.*$') | ForEach-Object { $_.Value.TrimEnd("`r") })
    if ($entries.Count -ne 1) { throw "Cargo.toml 必须恰有一个 lyflow-client 依赖，实际 $($entries.Count) 个。" }
    if ($entries[0] -cnotmatch '^\s*lyflow-client\s*=\s*\{\s*git\s*=\s*"([^"]+)"\s*,\s*tag\s*=\s*"([^"]+)"\s*\}\s*$') {
        throw 'Cargo.toml 的 lyflow-client 必须写成 { git = "...", tag = "vX.Y.Z" }，不得使用 rev、branch 或其他字段。'
    }
    $url = $Matches[1]
    $tag = $Matches[2]
    if ($url -cne (Get-LyFlowGitUrl $Config)) { throw "lyflow-client 的 git 地址必须是 $(Get-LyFlowGitUrl $Config)：$url" }
    if ($tag -cnotmatch '^v(.+)$' -or -not (Test-LyFlowSeriesVersion $Matches[1] $Config.lyflow.series)) {
        throw "lyflow-client 的 tag $tag 不属于 packaging/runtime.json 的 $($Config.lyflow.series) 系列。"
    }
    $tag
}

function Get-LyFlowReleases([string]$Repository) {
    $gh = Get-Command gh -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($gh) {
        & $gh.Source auth status *> $null
        if ($LASTEXITCODE -eq 0) {
            $output = @(& $gh.Source api --paginate "repos/$Repository/releases?per_page=100" --jq '.[] | {tag_name, draft, prerelease, assets: [.assets[] | {name, browser_download_url}]}' 2>&1)
            if ($LASTEXITCODE -eq 0) {
                return @($output | Where-Object { $_ -is [string] -and $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
            }
            Write-Warning "gh api 查询 lyFlow Release 失败，改用匿名 GitHub API：$(($output | Select-Object -Last 1))"
        }
    }
    $releases = [Collections.Generic.List[object]]::new()
    $headers = @{ Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
    for ($page = 1; $page -le 20; $page++) {
        $batch = @(Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases?per_page=100&page=$page" -Headers $headers -TimeoutSec 60)
        foreach ($release in $batch) { $releases.Add($release) }
        if ($batch.Count -lt 100) { break }
    }
    $releases.ToArray()
}

function Select-LyFlowRelease($Releases, $Config) {
    $series = $Config.lyflow.series
    $prefix = "https://github.com/$($Config.lyflow.repository)/releases/download/"
    $candidates = foreach ($release in @($Releases)) {
        if ((Get-RuntimeField $release 'draft') -ne $false -or (Get-RuntimeField $release 'prerelease') -ne $false) { continue }
        $tag = Get-RuntimeField $release 'tag_name'
        if ($tag -isnot [string] -or $tag -cnotmatch '^v(.+)$' -or -not (Test-LyFlowSeriesVersion $Matches[1] $series)) { continue }
        $version = $tag.Substring(1)
        $assetName = "lyflow-runtime-win-x64-v$version.zip"
        $asset = @(@(Get-RuntimeField $release 'assets') | Where-Object { $null -ne $_ -and (Get-RuntimeField $_ 'name') -ceq $assetName }) | Select-Object -First 1
        if (-not $asset) { continue }
        $url = Get-RuntimeField $asset 'browser_download_url'
        if ($url -isnot [string] -or -not $url.StartsWith($prefix, [StringComparison]::Ordinal)) { continue }
        [pscustomobject]@{ version = $version; tag = $tag; assetName = $assetName; downloadUrl = $url }
    }
    $selected = @($candidates | Sort-Object { [version]$_.version } -Descending) | Select-Object -First 1
    if (-not $selected) { throw "$($Config.lyflow.repository) 没有 $series 系列带 Windows 运行库资产的正式 Release。" }
    $selected
}

function Save-LyFlowReleaseAsset($Release, [string]$Destination) {
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $Release.downloadUrl -OutFile $Destination -TimeoutSec 900 -MaximumRetryCount 2 -RetryIntervalSec 3
}

function Get-LyFlowDllMetadata([string]$RuntimeRoot) {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.ComponentModel;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
public static class LyFlowDependencyProbe {
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32", CharSet=CharSet.Ansi, ExactSpelling=true)] static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32", SetLastError=true)] static extern bool SetDefaultDllDirectories(uint flags);
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr AddDllDirectory(string path);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr ReadString();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void FreeString(IntPtr value);
    public static IntPtr Load(string root) {
        if (!SetDefaultDllDirectories(0xC00)) throw new Win32Exception(Marshal.GetLastWin32Error());
        foreach (string directory in new[]{root}) if (AddDllDirectory(directory) == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        foreach (string directory in Directory.GetDirectories(root, "*", SearchOption.AllDirectories)) if (AddDllDirectory(directory) == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        var module = LoadLibraryExW(Path.Combine(root, "lyflow_core.dll"), IntPtr.Zero, 0xD00);
        if (module == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        foreach (string name in new[]{"lyflow_run_start","lyflow_run_join","lyflow_run_free","lyflow_run_summary","lyflow_run_outputs","lyflow_validate","lyflow_cache_clear"}) Function(module, name);
        return module;
    }
    static IntPtr Function(IntPtr module, string name) {
        var function = GetProcAddress(module, name);
        if (function == IntPtr.Zero) throw new Exception("lyFlow 缺少接口：" + name);
        return function;
    }
    public static string Read(IntPtr module, string name, bool owned) {
        var value = Marshal.GetDelegateForFunctionPointer<ReadString>(Function(module, name))();
        if (value == IntPtr.Zero) throw new Exception("lyFlow 返回空字符串：" + name);
        try { return Marshal.PtrToStringUTF8(value); }
        finally { if (owned) Marshal.GetDelegateForFunctionPointer<FreeString>(Function(module, "lyflow_string_free"))(value); }
    }
    public static void CheckModules(string root) {
        var packaged = Directory.GetFiles(root, "*", SearchOption.AllDirectories).Select(Path.GetFileName).ToHashSet(StringComparer.OrdinalIgnoreCase);
        string prefix = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        foreach (ProcessModule module in Process.GetCurrentProcess().Modules) {
            string path = module.FileName;
            if ((packaged.Contains(Path.GetFileName(path)) || Regex.IsMatch(Path.GetFileName(path), @"^(msvcr|msvcp|vcruntime)\d+(_\d+)?\.dll$", RegexOptions.IgnoreCase)) && !Path.GetFullPath(path).StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) throw new Exception("lyFlow 运行库被包外模块掩盖：" + path);
        }
    }
}
'@
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $core = [LyFlowDependencyProbe]::Load($RuntimeRoot)
    $problems = [LyFlowDependencyProbe]::Read($core, 'lyflow_manifest_problems', $true)
    if ($problems.Trim()) { throw "lyFlow 注册表自检失败：$problems" }
    [LyFlowDependencyProbe]::CheckModules($RuntimeRoot)
    [pscustomobject]@{
        version = [LyFlowDependencyProbe]::Read($core, 'lyflow_version', $false)
        abiVersion = [LyFlowDependencyProbe]::Read($core, 'lyflow_abi_version_string', $false)
        operators = @(([LyFlowDependencyProbe]::Read($core, 'lyflow_manifest_json', $true) | ConvertFrom-Json).operators.id)
    }
}

function Invoke-LyFlowDllProbe([string]$RuntimeRoot) {
    $payload = @($script:LyFlowDependencyScriptPath, $RuntimeRoot) | ConvertTo-Json -Compress
    $encodedPayload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $command = "`$p = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$encodedPayload')) | ConvertFrom-Json; . `$p[0]; Get-LyFlowDllMetadata `$p[1] | ConvertTo-Json -Depth 5 -Compress"
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoLogo', '-NoProfile', '-EncodedCommand', [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command)))) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(90000)) { $process.Kill($true); throw 'lyFlow DLL 验证超过 90 秒。' }
        $output = $stdout.GetAwaiter().GetResult()
        $errors = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "lyFlow DLL 加载或实际接口验证失败：$errors" }
        $output | ConvertFrom-Json
    } finally { $process.Dispose() }
}

function Test-LyFlowRuntimeDirectory([string]$RuntimeRoot, $Config, [string]$Version) {
    $series = $Config.lyflow.series
    $descriptorPath = Join-Path $RuntimeRoot 'lyflow-runtime.json'
    if (-not (Test-Path -LiteralPath $descriptorPath -PathType Leaf)) { throw "lyFlow 运行库缺少 lyflow-runtime.json：$RuntimeRoot" }
    $descriptor = Get-Content -LiteralPath $descriptorPath -Raw | ConvertFrom-Json
    foreach ($field in @('engineVersion', 'abiVersion')) {
        $value = Get-RuntimeField $descriptor $field
        if (-not (Test-LyFlowSeriesVersion $value $series)) { throw "lyFlow 运行库描述的 $field '$value' 不属于 $series 系列。" }
    }
    if ($descriptor.engineVersion -cne $Version) { throw "lyFlow 运行库描述的引擎版本 $($descriptor.engineVersion) 与所选版本 $Version 不一致。" }
    if ((Get-RuntimeField $descriptor 'architecture') -cne 'x64') { throw 'lyFlow 运行库描述的架构必须是 x64。' }
    Assert-LyFlowReleaseMetadata $descriptor 'lyFlow 运行库描述'
    $files = @(Get-RuntimeField $descriptor 'files')
    if (-not $files.Count) { throw 'lyFlow 运行库描述没有文件清单。' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $files) {
        $path = Get-RuntimeField $entry 'path'
        if ($path -isnot [string] -or -not $path -or $path.Contains('\') -or $path.Contains(':') -or
            $path.StartsWith('/') -or $path.Split('/') -contains '..' -or $path.Split('/') -contains '.' -or -not $seen.Add($path)) {
            throw "lyFlow 运行库文件清单路径无效或重复：$path"
        }
        if (-not (Test-Path -LiteralPath (Join-Path $RuntimeRoot $path) -PathType Leaf)) { throw "lyFlow 运行库清单文件缺失：$path" }
    }
    if (-not $seen.Contains('lyflow_core.dll')) { throw 'lyFlow 运行库文件清单缺少 lyflow_core.dll。' }
    $declared = @(@(Get-RuntimeField $descriptor 'operators') | ForEach-Object { Get-RuntimeField $_ 'id' })
    foreach ($operator in $Config.requiredOperators) {
        if ($operator -cnotin $declared) { throw "lyFlow 运行库描述缺少必需算子：$operator" }
    }
    foreach ($file in Get-ChildItem -LiteralPath $RuntimeRoot -File -Recurse -Force | Where-Object Extension -eq '.dll') { Assert-X64RuntimeImage $file.FullName }
    $actual = Invoke-LyFlowDllProbe $RuntimeRoot
    if ($actual.version -cne $Version) { throw "lyFlow 实际 DLL 引擎版本 $($actual.version) 与所选版本 $Version 不一致。" }
    if (-not (Test-LyFlowSeriesVersion $actual.abiVersion $series)) { throw "lyFlow 实际 DLL ABI 版本 $($actual.abiVersion) 不属于 $series 系列。" }
    foreach ($operator in $Config.requiredOperators) {
        if ($operator -cnotin @($actual.operators)) { throw "lyFlow 实际 DLL 缺少必需算子：$operator" }
    }
    $descriptor
}

function Get-LyFlowCacheRoot([string]$RepositoryRoot) {
    Join-Path ([IO.Path]::GetFullPath($RepositoryRoot)) 'src-tauri/target/dependency-cache/lyflow'
}

function Remove-LyFlowCacheDirectory([string]$Path, [string]$CacheRoot) {
    $absolute = [IO.Path]::GetFullPath($Path)
    $prefix = [IO.Path]::GetFullPath($CacheRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $absolute.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝删除 lyFlow 缓存目录以外的路径。' }
    if (Test-Path -LiteralPath $absolute) { Remove-Item -LiteralPath $absolute -Recurse -Force }
}

function Get-LyFlowCachedFallback([string]$CacheRoot, $Config, [string]$Reason) {
    $series = $Config.lyflow.series
    $cached = @(Get-ChildItem -LiteralPath $CacheRoot -Directory | Where-Object { Test-LyFlowSeriesVersion $_.Name $series } | Sort-Object { [version]$_.Name } -Descending)
    foreach ($directory in $cached) {
        try { [void](Test-LyFlowRuntimeDirectory $directory.FullName $Config $directory.Name) }
        catch { Write-Warning "跳过未通过校验的 lyFlow 缓存 $($directory.Name)：$($_.Exception.Message)"; continue }
        Write-Warning "$Reason；改用本机已缓存的 $series 系列最高版本 $($directory.Name)，它可能不是最新 Release。"
        return [pscustomobject]@{ version = $directory.Name; path = $directory.FullName; source = 'cache-fallback' }
    }
    throw "$Reason；本机也没有 $series 系列校验通过的 lyFlow 缓存。"
}

function Restore-LyFlowDependency([string]$RepositoryRoot, $Config, [switch]$NoFallback) {
    $cacheRoot = Get-LyFlowCacheRoot $RepositoryRoot
    [void][IO.Directory]::CreateDirectory($cacheRoot)
    try { $releases = Get-LyFlowReleases $Config.lyflow.repository }
    catch {
        $reason = "查询 $($Config.lyflow.repository) 的 Release 失败：$($_.Exception.Message)"
        if ($NoFallback) { throw $reason }
        return Get-LyFlowCachedFallback $cacheRoot $Config $reason
    }
    $release = Select-LyFlowRelease $releases $Config
    $target = Join-Path $cacheRoot $release.version
    if (Test-Path -LiteralPath $target) {
        try {
            [void](Test-LyFlowRuntimeDirectory $target $Config $release.version)
            Write-Host "lyFlow $($release.version) 缓存校验通过，跳过下载。"
            return [pscustomobject]@{ version = $release.version; path = $target; source = 'cache' }
        } catch {
            Write-Warning "lyFlow $($release.version) 缓存未通过校验，删除后重新下载：$($_.Exception.Message)"
            Remove-LyFlowCacheDirectory $target $cacheRoot
        }
    }
    $temporary = Join-Path $cacheRoot ".download-$([guid]::NewGuid().ToString('N'))"
    [void][IO.Directory]::CreateDirectory($temporary)
    try {
        $archive = Join-Path $temporary $release.assetName
        Write-Host "下载 lyFlow $($release.tag)：$($release.downloadUrl)"
        try { Save-LyFlowReleaseAsset $release $archive }
        catch {
            $reason = "下载 lyFlow $($release.tag) 失败：$($_.Exception.Message)"
            if ($NoFallback) { throw $reason }
            return Get-LyFlowCachedFallback $cacheRoot $Config $reason
        }
        $expanded = Join-Path $temporary 'runtime'
        Expand-RuntimeArchive $archive $expanded
        [void](Test-LyFlowRuntimeDirectory $expanded $Config $release.version)
        if (Test-Path -LiteralPath $target) { throw 'lyFlow 缓存被并发写入，请重试。' }
        Move-Item -LiteralPath $expanded -Destination $target
        Write-Host "lyFlow $($release.version) 下载并校验通过，已写入缓存。"
        [pscustomobject]@{ version = $release.version; path = $target; source = 'download' }
    } finally { Remove-LyFlowCacheDirectory $temporary $cacheRoot }
}

function Invoke-LyFlowCargoUpdate {
    & cargo update -p lyflow-client
    if ($LASTEXITCODE -ne 0) { throw "cargo update -p lyflow-client 失败（退出码 $LASTEXITCODE）。" }
}

function Set-LyFlowDependencyFiles([string]$RepositoryRoot, [string]$Series, [string]$Tag) {
    $paths = @((Join-Path $RepositoryRoot 'packaging/runtime.json'), (Join-Path $RepositoryRoot 'src-tauri/Cargo.toml'), (Join-Path $RepositoryRoot 'src-tauri/Cargo.lock'))
    $backups = @($paths | ForEach-Object { ,([IO.File]::ReadAllBytes($_)) })
    try {
        $runtime = [IO.File]::ReadAllText($paths[0])
        $seriesPattern = '("series"\s*:\s*")[^"]*(")'
        if ([regex]::Matches($runtime, $seriesPattern).Count -ne 1) { throw 'packaging/runtime.json 必须恰有一个 series 字段。' }
        [IO.File]::WriteAllText($paths[0], [regex]::Replace($runtime, $seriesPattern, [Text.RegularExpressions.MatchEvaluator]{ param($match) $match.Groups[1].Value + $Series + $match.Groups[2].Value }))
        $config = Get-LyFlowDependencyConfig $RepositoryRoot
        $cargo = [IO.File]::ReadAllText($paths[1])
        $tagPattern = '(?m)^([ \t]*lyflow-client[ \t]*=[^\r\n]*\btag[ \t]*=[ \t]*")[^"]*(")'
        if ([regex]::Matches($cargo, $tagPattern).Count -ne 1) { throw 'Cargo.toml 必须恰有一个使用 tag 的 lyflow-client 依赖。' }
        [IO.File]::WriteAllText($paths[1], [regex]::Replace($cargo, $tagPattern, [Text.RegularExpressions.MatchEvaluator]{ param($match) $match.Groups[1].Value + $Tag + $match.Groups[2].Value }))
        if ((Get-LyFlowCargoTag $paths[1] $config) -cne $Tag) { throw 'Cargo.toml 的 lyflow-client tag 写入失败。' }
        Push-Location -LiteralPath (Join-Path $RepositoryRoot 'src-tauri')
        try { Invoke-LyFlowCargoUpdate } finally { Pop-Location }
        $lock = [IO.File]::ReadAllText($paths[2])
        $source = 'source = "git+' + (Get-LyFlowGitUrl $config) + '?tag=' + $Tag + '#'
        if (-not $lock.Contains($source)) { throw "Cargo.lock 的 lyflow-client 没有切换到 $Tag。" }
    } catch {
        for ($index = 0; $index -lt $paths.Count; $index++) { [IO.File]::WriteAllBytes($paths[$index], $backups[$index]) }
        throw
    }
}
