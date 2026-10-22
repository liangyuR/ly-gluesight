param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9337,
    [switch]$SkipBuild,
    [switch]$ResetData,
    [ValidateSet('ui', 'p0', 'flow', 'e2e', 'soak')][string]$Profile = 'ui',
    [string]$DataRoot
)

$ErrorActionPreference = 'Stop'
$taskUiRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskUiOutput = Join-Path $taskUiRoot "output\playwright\$Profile-regression"
# 隔离实例的 Tauri 覆盖配置按 Profile 生成（各 Profile 只差名称与标识），构建时写到输出目录供 tauri build --config 读取。
$taskUiConfig = Join-Path $taskUiOutput 'tauri-test-config.json'
$taskUiExecutable = Join-Path $taskUiOutput 'gluesight-ui-tests.exe'
$taskUiTarget = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $taskUiRoot 'src-tauri\target' }
$taskUiBuild = Join-Path $taskUiTarget 'debug\GlueSight.exe'
$taskUiManifest = Join-Path $taskUiOutput 'instance.json'
$taskUiStdout = Join-Path $taskUiOutput 'stdout.log'
$taskUiStderr = Join-Path $taskUiOutput 'stderr.log'
$taskUiExisting = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $taskUiExecutable })
if ($taskUiExisting.Count) { throw "隔离 UI 实例仍在运行：$($taskUiExisting.Id -join ', ')。请先完成或关闭该实例。" }
$taskUiTitle = "GlueSight · 胶路智检 $(@{ ui = 'UI'; p0 = 'P0'; flow = 'Flow'; e2e = 'E2E'; soak = 'Soak' }[$Profile]) Tests"
$taskUiOverlay = [ordered]@{
    productName = $taskUiTitle
    identifier = "com.xyzrobotics.tujiaovision.$Profile-tests"
    build = [ordered]@{ beforeBuildCommand = 'node node_modules/typescript/bin/tsc -b && node node_modules/vite/bin/vite.js build' }
    app = [ordered]@{ windows = @([ordered]@{ label = 'main'; title = $taskUiTitle; visible = $false; width = 1440; height = 900; minWidth = 1100; minHeight = 700 }) }
    bundle = [ordered]@{ active = $false }
}
if ($taskUiOverlay.identifier -ne "com.xyzrobotics.tujiaovision.$Profile-tests" -or $taskUiOverlay.app.windows[0].visible) {
    throw 'UI test configuration must use the isolated application identifier and a hidden window.'
}
$taskUiIdentifier = "com.xyzrobotics.tujiaovision.$Profile-tests"
$taskUiDataParent = if ($DataRoot) { $DataRoot } elseif ($env:GLUESIGHT_TEST_DATA_ROOT) { $env:GLUESIGHT_TEST_DATA_ROOT } else { $taskUiOutput }
if (![IO.Path]::IsPathRooted($taskUiDataParent)) { throw "测试资料根目录必须是绝对路径：$taskUiDataParent" }
$taskUiDataParent = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($taskUiDataParent))
$taskUiData = Join-Path $taskUiDataParent $taskUiIdentifier
if ($ResetData -and (Test-Path -LiteralPath $taskUiData)) {
    $taskUiItem = Get-Item -LiteralPath $taskUiData -Force
    if (!$taskUiItem.PSIsContainer -or ($taskUiItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $taskUiItem.Name -cne $taskUiIdentifier -or
        [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($taskUiItem.Parent.FullName)) -ne $taskUiDataParent) {
        throw "拒绝清理非隔离测试数据目录：$taskUiData"
    }
    $taskUiAncestor = Get-Item -LiteralPath $taskUiItem.Parent.FullName
    while ($null -ne $taskUiAncestor) {
        if ($taskUiAncestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw '隔离测试目录的父路径包含链接，拒绝清理。' }
        $taskUiAncestor = $taskUiAncestor.Parent
    }
    Remove-Item -LiteralPath $taskUiItem.FullName -Recurse -Force
}
New-Item -ItemType Directory -Path $taskUiOutput -Force | Out-Null
New-Item -ItemType Directory -Path $taskUiDataParent -Force | Out-Null

if (!('GlueSightTest.IsolatedLauncher' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace GlueSightTest {
    public static class IsolatedLauncher {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        struct StartupInfo {
            public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
            public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
            public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct StartupInfoEx { public StartupInfo StartupInfo; public IntPtr lpAttributeList; }
        [StructLayout(LayoutKind.Sequential)]
        struct ProcessInformation { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
        [StructLayout(LayoutKind.Sequential)]
        struct SecurityAttributes { public int nLength; public IntPtr lpSecurityDescriptor; public int bInheritHandle; }

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previous, IntPtr returned);
        [DllImport("kernel32.dll")]
        static extern void DeleteProcThreadAttributeList(IntPtr list);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern bool CreateProcessW(string application, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes,
            bool inheritHandles, uint flags, IntPtr environment, string directory, ref StartupInfoEx startup, out ProcessInformation information);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern SafeFileHandle CreateFileW(string name, uint access, uint share, ref SecurityAttributes security, uint disposition, uint flags, IntPtr template);
        [DllImport("kernel32.dll")]
        static extern bool CloseHandle(IntPtr handle);

        static SafeFileHandle Open(string path, bool write) {
            var security = new SecurityAttributes { nLength = Marshal.SizeOf<SecurityAttributes>(), bInheritHandle = 1 };
            var handle = CreateFileW(path, write ? 0x40000000u : 0x80000000u, 3, ref security, write ? 2u : 3u, 0x80, IntPtr.Zero);
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), path);
            return handle;
        }

        // 子进程只继承 NUL 输入与两个日志文件句柄，不继承调用方的管道，调用方读到 EOF 后即可返回。
        public static int Start(string executable, string directory, string stdout, string stderr) {
            using (var input = Open("NUL", false))
            using (var output = Open(stdout, true))
            using (var error = Open(stderr, true)) {
                var handles = new[] { input.DangerousGetHandle(), output.DangerousGetHandle(), error.DangerousGetHandle() };
                var size = IntPtr.Zero;
                InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
                var list = Marshal.AllocHGlobal(size);
                var pinned = GCHandle.Alloc(handles, GCHandleType.Pinned);
                try {
                    if (!InitializeProcThreadAttributeList(list, 1, 0, ref size)) throw new Win32Exception();
                    try {
                        if (!UpdateProcThreadAttribute(list, 0, (IntPtr)0x20002, pinned.AddrOfPinnedObject(), (IntPtr)(IntPtr.Size * handles.Length), IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception();
                        var startup = new StartupInfoEx();
                        startup.StartupInfo.cb = Marshal.SizeOf<StartupInfoEx>();
                        startup.StartupInfo.dwFlags = 0x101;
                        startup.StartupInfo.hStdInput = handles[0];
                        startup.StartupInfo.hStdOutput = handles[1];
                        startup.StartupInfo.hStdError = handles[2];
                        startup.lpAttributeList = list;
                        ProcessInformation information;
                        if (!CreateProcessW(executable, new StringBuilder("\"" + executable + "\""), IntPtr.Zero, IntPtr.Zero, true,
                            0x08080000, IntPtr.Zero, directory, ref startup, out information)) throw new Win32Exception();
                        CloseHandle(information.hThread);
                        CloseHandle(information.hProcess);
                        return information.dwProcessId;
                    } finally { DeleteProcThreadAttributeList(list); }
                } finally { pinned.Free(); Marshal.FreeHGlobal(list); }
            }
        }
    }
}
'@
}

function Get-TaskUiPages([int]$port) {
    foreach ($cdpHost in '127.0.0.1', '[::1]') {
        try { $targets = Invoke-RestMethod -Uri "http://${cdpHost}:$port/json/list" -TimeoutSec 2 } catch { continue }
        foreach ($target in @($targets)) {
            if ($target.type -eq 'page') { [pscustomobject]@{ host = $cdpHost; url = [string]$target.url } }
        }
    }
}

function Get-TaskUiStderr {
    if (!(Test-Path -LiteralPath $taskUiStderr)) { return '' }
    ((Get-Content -LiteralPath $taskUiStderr -Tail 20 -ErrorAction SilentlyContinue) -join "`n").Trim()
}

Push-Location -LiteralPath $taskUiRoot
try {
    if (!$SkipBuild) {
        $taskUiOverlay | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $taskUiConfig -Encoding utf8NoBOM
        & node (Join-Path $taskUiRoot 'node_modules\@tauri-apps\cli\tauri.js') build --debug --no-bundle --config $taskUiConfig
        if ($LASTEXITCODE -ne 0) { throw '隔离 UI 构建失败' }
        Copy-Item -LiteralPath $taskUiBuild -Destination $taskUiExecutable -Force
    } elseif (!(Test-Path -LiteralPath $taskUiExecutable -PathType Leaf)) {
        throw 'SkipBuild 需要已有的隔离测试可执行文件。'
    }
    if ($SkipBuild) {
        $taskUiPrevious = Get-Content -LiteralPath $taskUiManifest -Raw | ConvertFrom-Json
        if ($taskUiPrevious.identifier -ne $taskUiOverlay.identifier -or $taskUiPrevious.executable -ne $taskUiExecutable) {
            throw '隔离测试实例标识或路径不匹配，请重新构建。'
        }
    }
    foreach ($taskUiAddress in [Net.IPAddress]::Loopback, [Net.IPAddress]::IPv6Loopback) {
        $taskUiProbe = [Net.Sockets.TcpListener]::new($taskUiAddress, $DebugPort)
        try { $taskUiProbe.Start() } catch [Net.Sockets.SocketException] {
            if ($_.Exception.SocketErrorCode -in 'AddressAlreadyInUse', 'AccessDenied') { throw "调试端口 $DebugPort 不可用（$taskUiAddress），请换 -DebugPort。" }
        } finally { $taskUiProbe.Stop() }
    }
    $taskUiOldArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    $taskUiOldData = $env:GLUESIGHT_DATA_DIR
    try {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort"
        $env:GLUESIGHT_DATA_DIR = $taskUiData
        $taskUiPid = [GlueSightTest.IsolatedLauncher]::Start($taskUiExecutable, $taskUiRoot, $taskUiStdout, $taskUiStderr)
    } finally {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $taskUiOldArguments
        $env:GLUESIGHT_DATA_DIR = $taskUiOldData
    }
    $taskUiProcess = Get-Process -Id $taskUiPid -ErrorAction SilentlyContinue
    if (!$taskUiProcess) { throw "隔离 UI 程序启动后立即退出。`n$(Get-TaskUiStderr)" }
    $null = $taskUiProcess.Handle
    try {
        $taskUiState = [ordered]@{ pid = $taskUiProcess.Id; executable = $taskUiExecutable; identifier = $taskUiOverlay.identifier;
            dataRoot = $taskUiData; debugPort = $DebugPort; cdp = $null; stdout = $taskUiStdout; stderr = $taskUiStderr;
            startedAt = $taskUiProcess.StartTime.ToUniversalTime().ToString('o') }
        $taskUiState | ConvertTo-Json | Set-Content -LiteralPath $taskUiManifest -Encoding utf8
        $taskUiDeadline = [DateTime]::UtcNow.AddSeconds(120)
        while (!$taskUiState.cdp) {
            $taskUiPages = @(Get-TaskUiPages $DebugPort)
            $taskUiMain = @($taskUiPages | Where-Object { $_.url -match '^(http://tauri\.localhost|tauri://localhost)/' -and $_.url -notmatch 'storage-startup' })
            if ($taskUiMain.Count -eq 1 -and !($taskUiPages | Where-Object { $_.url -match 'storage-startup' })) {
                $taskUiState.cdp = "http://$($taskUiMain[0].host):$DebugPort"
                break
            }
            if ($taskUiProcess.HasExited) { throw "隔离 UI 程序在主窗口就绪前退出（退出码 $($taskUiProcess.ExitCode)）。`n$(Get-TaskUiStderr)" }
            if ([DateTime]::UtcNow -gt $taskUiDeadline) {
                throw "隔离 UI 程序 120 s 内未出现主窗口页面（调试端口 $DebugPort：$(@($taskUiPages | ForEach-Object { "$($_.host) $($_.url)" }) -join '；')）。日志见 $taskUiOutput。`n$(Get-TaskUiStderr)"
            }
            Start-Sleep -Milliseconds 250
        }
        $taskUiState | ConvertTo-Json | Set-Content -LiteralPath $taskUiManifest -Encoding utf8
    } catch {
        $taskUiOwnedProcess = Get-Process -Id $taskUiProcess.Id -ErrorAction SilentlyContinue
        if ($taskUiOwnedProcess -and $taskUiOwnedProcess.Path -eq $taskUiExecutable) { Stop-Process -InputObject $taskUiOwnedProcess }
        throw
    }
    $taskUiState | ConvertTo-Json -Compress
} finally { Pop-Location }
