#Requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$script:LyFlowDependencyScriptPath = $PSCommandPath

function Get-LyFlowDependencyConfig([string]$RepositoryRoot, [string]$RuntimeConfigPath) {
    if (-not $RuntimeConfigPath) { $RuntimeConfigPath = Join-Path $RepositoryRoot 'packaging/runtime.json' }
    $config = Get-Content -LiteralPath $RuntimeConfigPath -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $config
    if ($config.schemaVersion -ne 2 -or $config.architecture -cne 'x64') { throw 'lyFlow 依赖配置必须使用 schema 2 和 x64 架构。' }
    $dependency = $config.lyflow
    if ($dependency.source -notmatch '^https://' -or $dependency.packageId -cne 'LyFlow.Runtime.win-x64' -or
        $dependency.series -cnotmatch '^\d+\.\d+$') { throw 'lyFlow NuGet 来源、包 ID 或兼容系列无效。' }
    Assert-LyFlowCompatibleVersion $dependency.minimumVersion $dependency
    $config
}

function Assert-LyFlowCompatibleVersion($Version, $Dependency) {
    if ($Version -isnot [string] -or $Version -cnotmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') {
        throw "lyFlow 只接受完整正式版本，拒绝预发布或非法版本：$Version"
    }
    if (($Version -replace '\.\d+$', '') -cne $Dependency.series -or [version]$Version -lt [version]$Dependency.minimumVersion) {
        throw "lyFlow 版本 $Version 不属于允许的 $($Dependency.series) 系列或低于最低版本。"
    }
}

function Resolve-LyFlowPackageVersion([string[]]$Versions, $Dependency, [string]$Version) {
    if ($Version) { Assert-LyFlowCompatibleVersion $Version $Dependency; return $Version }
    $valid = @($Versions | Where-Object {
        try { Assert-LyFlowCompatibleVersion $_ $Dependency; $true } catch { $false }
    } | Sort-Object { [version]$_ } -Descending -Unique)
    if (-not $valid.Count) { throw 'NuGet 源中没有允许兼容系列的正式 lyFlow 包。' }
    $valid[0]
}

function Get-LyFlowDependencyLock([string]$RepositoryRoot, $Config, [string]$LockPath) {
    if (-not $LockPath) { $LockPath = Join-Path $RepositoryRoot 'packaging/lyflow.lock.json' }
    $locked = Get-Content -LiteralPath $LockPath -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $locked
    if ($locked.schemaVersion -ne 1 -or $locked.packageId -cne $Config.lyflow.packageId -or $locked.architecture -cne 'x64') {
        throw 'lyFlow 锁定记录 schema、包 ID 或架构无效。'
    }
    Assert-LyFlowCompatibleVersion $locked.version $Config.lyflow
    Assert-LyFlowSourceRevision $locked.sourceRevision $locked.sourceRevision '锁定记录'
    $locked
}

function Get-NuGetTool([string]$RepositoryRoot) {
    $path = Join-Path $RepositoryRoot 'src-tauri/target/dependency-tools/nuget-6.14.0.exe'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))
        $partial = "$path.$([guid]::NewGuid().ToString('N')).partial"
        try {
            Invoke-WebRequest -Uri 'https://dist.nuget.org/win-x86-commandline/v6.14.0/nuget.exe' -OutFile $partial
            Assert-X64OrX86NuGetTool $partial
            Move-Item -LiteralPath $partial -Destination $path
        } finally { if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force } }
    }
    Assert-X64OrX86NuGetTool $path
    $path
}

function Assert-X64OrX86NuGetTool([string]$Path) {
    $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path).ProductVersion
    if ($version -notmatch '^6\.14\.0(?:[.+-]|$)') { throw 'NuGet CLI 必须是固定 6.14.0 版本。' }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
        throw 'NuGet CLI 的 Microsoft Authenticode 签名无效。'
    }
}

function Get-LyFlowNuGetCredential([string]$CredentialFile) {
    $token = $env:LYFLOW_NUGET_TOKEN
    $username = if ($env:LYFLOW_NUGET_USERNAME) { $env:LYFLOW_NUGET_USERNAME } else { 'liangyuR' }
    if (-not $token) {
        if (-not $CredentialFile) { $CredentialFile = Join-Path $env:USERPROFILE '.ssh/github_nuget_auth.txt' }
        if (Test-Path -LiteralPath $CredentialFile -PathType Leaf) {
            $credential = [IO.File]::ReadAllText($CredentialFile).Trim()
            if ($credential -match '(?m)(?:github_pat_[A-Za-z0-9_]+|ghp_[A-Za-z0-9]+)') { $token = $Matches[0] }
            elseif ($credential -notmatch '\s|=|:') { $token = $credential }
            else { throw '凭证文件缺少可识别的 GitHub token；可使用 LYFLOW_NUGET_TOKEN。' }
        }
    }
    if (-not $token) { throw '缺少 NuGet 读取凭证；请配置 LYFLOW_NUGET_TOKEN 或凭证文件。' }
    if ($username -match '[;\r\n]' -or $token -match '[;\r\n]') { throw 'NuGet 凭证格式无效。' }
    [pscustomobject]@{ username = $username; token = $token }
}

function Invoke-LyFlowNuGet([string]$RepositoryRoot, [string]$Source, [string[]]$Arguments, [string]$CredentialFile) {
    $tool = Get-NuGetTool $RepositoryRoot
    $credential = Get-LyFlowNuGetCredential $CredentialFile
    $token = $credential.token
    $username = $credential.username
    $directory = Join-Path $RepositoryRoot "src-tauri/target/dependency-tools/auth-$([guid]::NewGuid().ToString('N'))"
    [void][IO.Directory]::CreateDirectory($directory)
    $configPath = Join-Path $directory 'NuGet.Config'
    $escapedSource = [Security.SecurityElement]::Escape($Source)
    [IO.File]::WriteAllText($configPath, "<configuration><packageSources><clear/><add key=`"LyFlow`" value=`"$escapedSource`" /></packageSources></configuration>")
    $start = [Diagnostics.ProcessStartInfo]::new($tool)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['NuGetPackageSourceCredentials_LyFlow'] = "Username=$username;Password=$token;ValidAuthenticationTypes=Basic"
    $verbosity = if ($Arguments[0] -ceq 'list') { 'normal' } else { 'quiet' }
    foreach ($argument in @($Arguments) + @('-ConfigFile', $configPath, '-NonInteractive', '-Verbosity', $verbosity)) { $start.ArgumentList.Add($argument) }
    try {
        $process = [Diagnostics.Process]::Start($start)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $timeoutSeconds = 1200
            if ($env:LYFLOW_NUGET_TIMEOUT_SECONDS) {
                if (-not [int]::TryParse($env:LYFLOW_NUGET_TIMEOUT_SECONDS, [ref]$timeoutSeconds) -or $timeoutSeconds -lt 30 -or $timeoutSeconds -gt 3600) {
                    $process.Kill($true)
                    throw 'LYFLOW_NUGET_TIMEOUT_SECONDS 必须是 30 至 3600 秒。'
                }
            }
            $timer = [Diagnostics.Stopwatch]::StartNew()
            while (-not $process.WaitForExit([Math]::Min(30000, $timeoutSeconds * 1000))) {
                if ($timer.Elapsed.TotalSeconds -ge $timeoutSeconds) {
                    $process.Kill($true)
                    $process.WaitForExit()
                    throw "NuGet 请求超过 $timeoutSeconds 秒；大运行库下载可配置 LYFLOW_NUGET_TIMEOUT_SECONDS（最多 3600 秒）。"
                }
                Write-Host "NuGet 固定源请求进行中：已等待 $([int]$timer.Elapsed.TotalSeconds) 秒。"
            }
            $output = $stdout.GetAwaiter().GetResult()
            [void]$stderr.GetAwaiter().GetResult()
            if ($process.ExitCode -ne 0) { throw "NuGet 请求失败（退出码 $($process.ExitCode)）；请检查源、包版本和读取权限。" }
            $output.Replace($token, '[REDACTED]')
        } finally { $process.Dispose() }
    } finally {
        $token = $null
        $credential.token = $null
        $start.Environment.Remove('NuGetPackageSourceCredentials_LyFlow') | Out-Null
        Remove-Item -LiteralPath $configPath -Force
        Remove-Item -LiteralPath $directory -Force
    }
}

function Invoke-LyFlowNuGetMetadata([string]$Uri, [string]$Source, [string]$CredentialFile) {
    $sourceUri = [uri]$Source
    $requestUri = [uri]$Uri
    $ownerPath = $sourceUri.AbsolutePath.Substring(0, $sourceUri.AbsolutePath.LastIndexOf('/') + 1)
    if ($sourceUri.Scheme -cne 'https' -or $requestUri.Scheme -cne 'https' -or
        $requestUri.Authority -cne $sourceUri.Authority -or $requestUri.UserInfo -or
        -not $requestUri.AbsolutePath.StartsWith($ownerPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'NuGet 元数据端点必须使用相同 HTTPS 源及 owner 路径。'
    }
    $credential = Get-LyFlowNuGetCredential $CredentialFile
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $false
    $client = [Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(60)
    $authorization = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("$($credential.username):$($credential.token)"))
    $client.DefaultRequestHeaders.Authorization = [Net.Http.Headers.AuthenticationHeaderValue]::new('Basic', $authorization)
    try {
        $response = $client.GetAsync($requestUri).GetAwaiter().GetResult()
        try {
            if (-not $response.IsSuccessStatusCode) { throw "NuGet 元数据读取失败，HTTP $([int]$response.StatusCode)。" }
            $response.Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json
        } finally { $response.Dispose() }
    } finally {
        $credential.token = $null
        $authorization = $null
        $client.Dispose()
        $handler.Dispose()
    }
}

function Get-LyFlowAvailableVersions([string]$RepositoryRoot, $Config, [string]$CredentialFile) {
    $index = Invoke-LyFlowNuGetMetadata $Config.lyflow.source $Config.lyflow.source $CredentialFile
    $resources = @($index.resources | Where-Object { $_.'@type' -ceq 'PackageBaseAddress/3.0.0' })
    if ($resources.Count -ne 1) { throw 'NuGet v3 源必须声明唯一的 PackageBaseAddress/3.0.0。' }
    $baseAddress = $resources[0].'@id'.TrimEnd('/')
    $versions = Invoke-LyFlowNuGetMetadata "$baseAddress/$($Config.lyflow.packageId.ToLowerInvariant())/index.json" $Config.lyflow.source $CredentialFile
    @($versions.versions)
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
        sourceRevision = [LyFlowDependencyProbe]::Read($core, 'lyflow_source_revision', $false)
        operators = @(([LyFlowDependencyProbe]::Read($core, 'lyflow_manifest_json', $true) | ConvertFrom-Json).operators.id)
    }
}

function Invoke-LyFlowDllProbe([string]$RuntimeRoot) {
    $commonPath = $script:LyFlowDependencyScriptPath
    $payload = @($commonPath, $RuntimeRoot) | ConvertTo-Json -Compress
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

function Test-LyFlowRuntimeDirectory([string]$RuntimeRoot, $Config, $Lock) {
    Assert-NoRuntimeLinks $RuntimeRoot
    $descriptor = Get-Content -LiteralPath (Join-Path $RuntimeRoot 'lyflow-runtime.json') -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $descriptor
    if ($descriptor.schemaVersion -ne 1 -or $descriptor.architecture -cne 'x64' -or
        $descriptor.abiVersion -cne $Lock.version -or $descriptor.engineVersion -cne $Lock.version) {
        throw 'NuGet 运行库描述 schema、架构、ABI 或产品版本与锁定记录不符。'
    }
    Assert-LyFlowReleaseMetadata $descriptor 'NuGet 运行库描述'
    Assert-LyFlowSourceRevision $descriptor.sourceRevision $Lock.sourceRevision 'NuGet 运行库描述'
    $files = @(Get-ChildItem -LiteralPath $RuntimeRoot -File -Recurse -Force)
    if ($files.Count -ne @($descriptor.files).Count) { throw 'NuGet 运行库文件数量与清单不符。' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $descriptor.files) {
        $path = $entry.path
        if ($path -isnot [string] -or -not $path -or $path.Contains('\') -or $path.Contains(':') -or
            $path.StartsWith('/') -or $path.Split('/') -contains '..' -or $path.Split('/') -contains '.' -or -not $seen.Add($path)) {
            throw 'NuGet 运行库清单路径无效或重复。'
        }
        $file = Join-Path $RuntimeRoot $path
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "NuGet 运行库清单文件缺失：$path" }
        if ($entry.PSObject.Properties.Name -contains 'bytes' -and (Get-Item -LiteralPath $file).Length -ne $entry.bytes) { throw "NuGet 运行库文件尺寸不符：$path" }
    }
    foreach ($required in @('lyflow_core.dll', 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $RuntimeRoot $required) -PathType Leaf)) { throw "NuGet 运行库缺少 DLL：$required" }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $RuntimeRoot 'licenses') -PathType Container) -or
        -not @(Get-ChildItem -LiteralPath (Join-Path $RuntimeRoot 'licenses') -File -Recurse).Count) { throw 'NuGet 运行库许可证目录为空。' }
    foreach ($file in $files | Where-Object Extension -in @('.dll', '.exe', '.cti', '.ax')) { Assert-X64RuntimeImage $file.FullName }
    $actual = Invoke-LyFlowDllProbe $RuntimeRoot
    if ($actual.version -cne $Lock.version -or $actual.abiVersion -cne $Lock.version) { throw 'NuGet 实际 DLL ABI 或产品版本与锁定记录不符。' }
    Assert-LyFlowSourceRevision $actual.sourceRevision $Lock.sourceRevision 'NuGet 实际 DLL'
    foreach ($operator in $Config.requiredOperators) {
        if ($operator -notin @($actual.operators) -or $operator -notin @($descriptor.operators.id)) { throw "NuGet 运行库缺少必需算子：$operator" }
    }
    $descriptor
}

function Remove-LyFlowCacheDirectory([string]$Path, [string]$CacheRoot) {
    $absolute = [IO.Path]::GetFullPath($Path)
    $prefix = [IO.Path]::GetFullPath($CacheRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $absolute.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝删除 NuGet 缓存目录以外的路径。' }
    if (Test-Path -LiteralPath $absolute) { Assert-NoRuntimeLinks $absolute; Remove-Item -LiteralPath $absolute -Recurse -Force }
}

function Restore-LyFlowDependency([string]$RepositoryRoot, $Config, $Lock, [string]$CredentialFile) {
    Assert-LyFlowCompatibleVersion $Lock.version $Config.lyflow
    Assert-LyFlowSourceRevision $Lock.sourceRevision $Lock.sourceRevision '恢复锁定记录'
    $cacheRoot = Join-Path $RepositoryRoot 'src-tauri/target/dependency-cache/nuget'
    [void][IO.Directory]::CreateDirectory($cacheRoot)
    Assert-NoRuntimeLinks $cacheRoot
    $packageRoot = Join-Path $cacheRoot $Config.lyflow.packageId.ToLowerInvariant()
    [void][IO.Directory]::CreateDirectory($packageRoot)
    $cached = Join-Path $packageRoot $Lock.version
    $native = Join-Path $cached 'runtimes/win-x64/native'
    if (Test-Path -LiteralPath $cached) {
        try {
            [void](Test-LyFlowRuntimeDirectory $native $Config $Lock)
            Write-Host "lyFlow NuGet 缓存验证通过：$($Lock.version)"
            return $native
        } catch {
            Write-Host "lyFlow NuGet 缓存不完整或不符合锁定记录，重新获取固定版本 $($Lock.version)。"
            Remove-LyFlowCacheDirectory $cached $cacheRoot
        }
    }
    $temporary = Join-Path $cacheRoot ".restore-$([guid]::NewGuid().ToString('N'))"
    [void][IO.Directory]::CreateDirectory($temporary)
    try {
        [void](Invoke-LyFlowNuGet $RepositoryRoot $Config.lyflow.source @('install', $Config.lyflow.packageId, '-Version', $Lock.version, '-Source', 'LyFlow', '-OutputDirectory', $temporary, '-ExcludeVersion', '-DirectDownload', '-NoCache', '-PackageSaveMode', 'nupkg') $CredentialFile)
        $downloaded = Join-Path $temporary $Config.lyflow.packageId
        [void](Test-LyFlowRuntimeDirectory (Join-Path $downloaded 'runtimes/win-x64/native') $Config $Lock)
        Move-Item -LiteralPath $downloaded -Destination $cached
        Write-Host "lyFlow NuGet 固定版本恢复并验证通过：$($Lock.version)"
        $native
    } finally { Remove-LyFlowCacheDirectory $temporary $cacheRoot }
}

function Invoke-LyFlowCargoUpdate([string]$Revision) {
    & cargo update -p lyflow-client --precise $Revision
    if ($LASTEXITCODE -ne 0) { throw 'Cargo 客户端锁定更新失败，恢复原始依赖文件。' }
}

function Set-LyFlowDependencyFiles([string]$RepositoryRoot, $Lock) {
    $paths = @((Join-Path $RepositoryRoot 'packaging/lyflow.lock.json'), (Join-Path $RepositoryRoot 'src-tauri/Cargo.toml'), (Join-Path $RepositoryRoot 'src-tauri/Cargo.lock'))
    $backups = @($paths | ForEach-Object { ,([IO.File]::ReadAllBytes($_)) })
    $previousLock = [IO.File]::ReadAllText($paths[2])
    $plcPattern = '(?ms)^\[\[package\]\]\r?\nname = "ly-plc"\r?\n.*?(?=^\[\[package\]\]|\z)'
    $previousPlc = [regex]::Match($previousLock, $plcPattern).Value
    try {
        $cargo = [IO.File]::ReadAllText($paths[1])
        $pattern = '(?m)^(lyflow-client\s*=\s*\{[^\r\n]*\brev\s*=\s*")[0-9a-f]{40}("[^\r\n]*\})\s*$'
        if ([regex]::Matches($cargo, $pattern).Count -ne 1) { throw 'Cargo.toml 必须包含唯一的固定 lyflow-client Git rev。' }
        $revision = $Lock.sourceRevision
        $cargo = [regex]::Replace($cargo, $pattern, [Text.RegularExpressions.MatchEvaluator]{ param($match) $match.Groups[1].Value + $revision + $match.Groups[2].Value })
        [IO.File]::WriteAllText($paths[1], $cargo)
        Push-Location -LiteralPath (Join-Path $RepositoryRoot 'src-tauri')
        try {
            Invoke-LyFlowCargoUpdate $revision
            $lockText = [IO.File]::ReadAllText($paths[2])
            if ([regex]::Match($lockText, $plcPattern).Value -cne $previousPlc) { throw '更新 lyFlow 不允许改变 ly-plc 锁定记录。' }
            if ($lockText -notmatch ('(?s)name = "lyflow-client"\s+version = "' + [regex]::Escape($Lock.version) + '"\s+source = "[^"\r\n]*#' + $revision + '"')) {
                throw 'Cargo.lock 的 lyflow-client 版本或 Git 修订未与 NuGet 包配套。'
            }
        } finally { Pop-Location }
        $Lock | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $paths[0] -Encoding utf8NoBOM
    } catch {
        for ($index = 0; $index -lt $paths.Count; $index++) { [IO.File]::WriteAllBytes($paths[$index], $backups[$index]) }
        throw
    }
}
