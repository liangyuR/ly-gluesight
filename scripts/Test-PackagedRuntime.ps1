#Requires -Version 7.0
param([Parameter(Mandatory)][string]$RuntimeDir)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class PackagedRuntimeProbe {
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32", SetLastError = true)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32")]
    public static extern bool FreeLibrary(IntPtr module);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    delegate IntPtr GetString();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    delegate void FreeString(IntPtr value);
    public static IntPtr Load(string path) {
        var handle = LoadLibraryExW(path, IntPtr.Zero, 0x00000900);
        if (handle == IntPtr.Zero) {
            int code = Marshal.GetLastWin32Error();
            throw new Win32Exception(code, path + ": " + new Win32Exception(code).Message);
        }
        return handle;
    }
    public static string Read(IntPtr module, string name) {
        var getter = GetProcAddress(module, name);
        var freer = GetProcAddress(module, "lyflow_string_free");
        if (getter == IntPtr.Zero || freer == IntPtr.Zero) throw new Exception("核心库缺少接口：" + name);
        var value = Marshal.GetDelegateForFunctionPointer<GetString>(getter)();
        if (value == IntPtr.Zero) throw new Exception("核心库返回空指针：" + name);
        try { return Marshal.PtrToStringUTF8(value); }
        finally { Marshal.GetDelegateForFunctionPointer<FreeString>(freer)(value); }
    }
}
'@
$RuntimeDir = (Resolve-Path -LiteralPath $RuntimeDir).Path
$core = [PackagedRuntimeProbe]::Load((Join-Path $RuntimeDir 'lyflow_core.dll'))
try {
    $problems = [PackagedRuntimeProbe]::Read($core, 'lyflow_manifest_problems')
    if ($problems.Trim()) { throw "lyFlow 自检失败：$problems" }
    $manifest = [PackagedRuntimeProbe]::Read($core, 'lyflow_manifest_json') | ConvertFrom-Json
    foreach ($operator in @('io.load_image', 'image.board_calib', 'image.load_calib', 'glue.locate', 'glue.station_calipers')) {
        if ($operator -notin $manifest.operators.id) { throw "lyFlow 缺少必需算子：$operator" }
    }
    foreach ($dll in Get-ChildItem -LiteralPath $RuntimeDir -Filter '*.dll' -File) {
        $handle = [PackagedRuntimeProbe]::Load($dll.FullName)
        [void][PackagedRuntimeProbe]::FreeLibrary($handle)
    }
    Write-Host '运行库自检通过：DLL 依赖可加载，飞拍和标定算子齐全。'
} finally { [void][PackagedRuntimeProbe]::FreeLibrary($core) }
