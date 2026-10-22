#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$RuntimeRoot,
    [Parameter(Mandatory)][string]$ReportPath,
    [switch]$ProbeProcess
)
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
Assert-RuntimePlatform
$RuntimeRoot = [IO.Path]::GetFullPath($RuntimeRoot)
$ReportPath = [IO.Path]::GetFullPath($ReportPath)
if (-not $ProbeProcess) {
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoLogo', '-NoProfile', '-File', $PSCommandPath, '-RuntimeRoot', $RuntimeRoot, '-ReportPath', $ReportPath, '-ProbeProcess')) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(90000)) {
            $process.Kill($true)
            $process.WaitForExit()
            [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($ReportPath))
            @{ success = $false; error = '运行库自检超过 90 秒，子进程已终止。' } | ConvertTo-Json | Set-Content -LiteralPath $ReportPath -Encoding utf8NoBOM
            throw "运行库自检超时；请查看 $ReportPath"
        }
        Write-Host $stdout.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) {
            Write-Host $stderr.GetAwaiter().GetResult()
            throw "随包运行库自检失败；请查看 $ReportPath"
        }
    } finally { $process.Dispose() }
    return
}
$report = [ordered]@{ success = $false; runtimeRoot = $RuntimeRoot; architecture = 'x64' }
try {
    $manifest = Assert-RuntimeManifest $RuntimeRoot
    $config = Get-RuntimeConfiguration
    Assert-MvsRuntimeStructure (Join-Path $RuntimeRoot 'mvs')
    foreach ($path in @('lyflow/lyflow_core.dll', 'lyflow/lyflow-runtime.json', 'lyflow/msvcp140.dll', 'lyflow/vcruntime140.dll', 'lyflow/vcruntime140_1.dll', 'mvs/MvCameraControl.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $RuntimeRoot $path) -PathType Leaf)) { throw "缺少必需运行库文件：$path" }
    }
    if (-not @(Get-ChildItem -LiteralPath (Join-Path $RuntimeRoot 'mvs/Licenses') -File -Recurse).Count) { throw 'MVS 许可证目录为空。' }
    $descriptor = Get-Content -LiteralPath (Join-Path $RuntimeRoot 'lyflow/lyflow-runtime.json') -Raw | ConvertFrom-Json
    Assert-LyFlowVersions $descriptor.abiVersion $descriptor.engineVersion '正式产物描述' $manifest.lyflow.version
    Assert-LyFlowReleaseMetadata $descriptor '正式产物描述'
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Linq;
using System.Diagnostics;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
public static class PackagedRuntimeProbe {
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32", CharSet=CharSet.Ansi, ExactSpelling=true, SetLastError=true)] static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32", SetLastError=true)] static extern bool SetDefaultDllDirectories(uint flags);
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr AddDllDirectory(string path);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr GetString();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void FreeString(IntPtr value);
    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate int MvsStatus();
    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate uint MvsVersion();
    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate int MvsEnum(uint mask, IntPtr list);
    public static T Function<T>(IntPtr module, string name) where T : Delegate {
        var pointer = GetProcAddress(module, name);
        if (pointer == IntPtr.Zero) throw new Exception("运行库缺少接口：" + name);
        return Marshal.GetDelegateForFunctionPointer<T>(pointer);
    }
    public static void Prepare(string root) {
        if (!SetDefaultDllDirectories(0xC00)) throw new Win32Exception(Marshal.GetLastWin32Error());
        foreach (string component in new[]{"lyflow", "mvs"}) {
            string path = Path.Combine(root, component);
            foreach (string directory in new[]{path}.Concat(Directory.GetDirectories(path, "*", SearchOption.AllDirectories))) {
                if (AddDllDirectory(directory) == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), directory);
            }
        }
    }
    public static IntPtr Load(string path) {
        var handle = LoadLibraryExW(path, IntPtr.Zero, 0xD00);
        if (handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), path);
        return handle;
    }
    public static string Read(IntPtr module, string name, bool owned=true) {
        IntPtr value = Function<GetString>(module, name)();
        if (value == IntPtr.Zero) throw new Exception("接口返回空指针：" + name);
        try { return Marshal.PtrToStringUTF8(value); }
        finally { if (owned) Function<FreeString>(module, "lyflow_string_free")(value); }
    }
    public static void Require(IntPtr module, string[] names) {
        foreach (string name in names) if (GetProcAddress(module, name) == IntPtr.Zero) throw new Exception("运行库缺少接口：" + name);
    }
    public static string ProbeMvs(IntPtr module, string runtimeRoot) {
        int init = Function<MvsStatus>(module, "MV_CC_Initialize")();
        if (init != 0) throw new Exception("MVS 初始化失败：0x" + init.ToString("X8"));
        try {
            uint version = Function<MvsVersion>(module, "MV_CC_GetSDKVersion")();
            IntPtr list = Marshal.AllocHGlobal(8 + 256 * IntPtr.Size);
            try {
                Marshal.Copy(new byte[8 + 256 * IntPtr.Size], 0, list, 8 + 256 * IntPtr.Size);
                int code = Function<MvsEnum>(module, "MV_CC_EnumDevices")(5, list);
                if (code != 0) throw new Exception("MVS 枚举失败：0x" + code.ToString("X8"));
            } finally { Marshal.FreeHGlobal(list); }
            return String.Format("{0}.{1}.{2}.{3}", version >> 24, (version >> 16) & 255, (version >> 8) & 255, version & 255);
        } finally {
            int final = Function<MvsStatus>(module, "MV_CC_Finalize")();
            if (final != 0) throw new Exception("MVS 结束失败：0x" + final.ToString("X8"));
        }
    }
}
'@
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    [PackagedRuntimeProbe]::Prepare($RuntimeRoot)
    $core = [PackagedRuntimeProbe]::Load((Join-Path $RuntimeRoot 'lyflow/lyflow_core.dll'))
    $abiVersion = [PackagedRuntimeProbe]::Read($core, 'lyflow_abi_version_string', $false)
    $version = [PackagedRuntimeProbe]::Read($core, 'lyflow_version', $false)
    Assert-LyFlowVersions $abiVersion $version '实际 DLL' $manifest.lyflow.version
    [PackagedRuntimeProbe]::Require($core, @('lyflow_run_start', 'lyflow_run_join', 'lyflow_run_free', 'lyflow_run_summary', 'lyflow_run_outputs', 'lyflow_validate', 'lyflow_cache_clear'))
    $problems = [PackagedRuntimeProbe]::Read($core, 'lyflow_manifest_problems')
    if ($problems.Trim()) { throw "lyFlow 注册表自检失败：$problems" }
    $coreManifest = [PackagedRuntimeProbe]::Read($core, 'lyflow_manifest_json') | ConvertFrom-Json
    $operators = $coreManifest.operators.id
    foreach ($operator in $config.requiredOperators) {
        if ($operator -notin $operators -or $operator -notin $manifest.lyflow.operators) { throw "lyFlow 缺少必需算子：$operator" }
    }
    # 带外圈模板的 glue.locate（lyFlow#32）：参数 templatePad、maxPeakRatio，输出 pose。缺少时按 locateSmoke 开关警告或失败
    $locateOp = @($coreManifest.operators | Where-Object { $_.id -ceq 'glue.locate' }) | Select-Object -First 1
    $hasItem = { param($list, $name) [bool](@(Get-RuntimeField $locateOp $list) | Where-Object { (Get-RuntimeField $_ 'name') -ceq $name }) }
    $locate = [ordered]@{ templatePad = (& $hasItem 'params' 'templatePad'); maxPeakRatio = (& $hasItem 'params' 'maxPeakRatio'); pose = (& $hasItem 'outputs' 'pose'); policy = (Get-LocateSmokePolicy $config) }
    if (-not ($locate.templatePad -and $locate.maxPeakRatio -and $locate.pose)) {
        $message = 'lyFlow 运行库的 glue.locate 不支持带外圈模板（templatePad / maxPeakRatio / pose）：含特征定位的配方不能发布，不能在生产加载。'
        if ($locate.policy -ceq 'required') { throw "$message packaging/runtime.json 的 lyflow.locateSmoke 为 required。" }
        Write-Warning "$message lyflow.locateSmoke 为 optional，只适用于没有特征定位的配方。"
    }
    $mvs = [PackagedRuntimeProbe]::Load((Join-Path $RuntimeRoot 'mvs/MvCameraControl.dll'))
    [PackagedRuntimeProbe]::Require($mvs, @('MV_CC_CreateHandle', 'MV_CC_OpenDevice', 'MV_CC_CloseDevice', 'MV_CC_DestroyHandle', 'MV_CC_StartGrabbing', 'MV_CC_StopGrabbing', 'MV_CC_GetOptimalPacketSize', 'MV_CC_RegisterImageCallBackEx', 'MV_CC_RegisterExceptionCallBack', 'MV_CC_SetEnumValueByString', 'MV_CC_SetFloatValue', 'MV_CC_GetFloatValue', 'MV_CC_SetIntValueEx', 'MV_CC_SetBoolValue', 'MV_CC_SetCommandValue', 'MV_CC_GetEnumValue', 'MV_CC_GetEnumEntrySymbolic'))
    $mvsVersion = [PackagedRuntimeProbe]::ProbeMvs($mvs, $RuntimeRoot)
    if ($mvsVersion -ne $config.mvsVersion) { throw "MVS 实际版本 $mvsVersion 与固定版本 $($config.mvsVersion) 不一致。" }
    $report.lyflow = @{ version = $version; abiVersion = $abiVersion; declaredAbiVersion = $descriptor.abiVersion; series = $config.lyflow.series; operators = @($operators); locate = $locate }
    $report.mvs = @{ version = $mvsVersion; initialization = 'passed'; enumeration = 'passed' }
    $report.success = $true
    Write-Host '运行库自检通过：文件清单、x64、接口、实际引擎版本与清单一致且属于锁定系列、必需算子及 MVS 初始化/枚举。客户端版本另由应用自检报告验证。'
} catch {
    $report.error = $_.Exception.Message
    Write-Host "运行库自检失败：$($report.error)"
} finally {
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($ReportPath))
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ReportPath -Encoding utf8NoBOM
}
if (-not $report.success) { exit 1 }
