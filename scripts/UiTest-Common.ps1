#Requires -Version 7.0
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding

function Get-UiTestLayout {
    param([ValidateSet('ui', 'p0', 'flow', 'e2e', 'soak')][string]$Profile = 'ui')
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $output = Join-Path $root "output/playwright/$Profile-regression"
    $target = if ($env:CARGO_TARGET_DIR) {
        if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) }
        else { [IO.Path]::GetFullPath((Join-Path $root "src-tauri/$env:CARGO_TARGET_DIR")) }
    } else { Join-Path $root 'src-tauri/target' }
    $title = "GlueSight · 胶路智检 $(@{ ui = 'UI'; p0 = 'P0'; flow = 'Flow'; e2e = 'E2E'; soak = 'Soak' }[$Profile]) Tests"
    $identifier = "com.xyzrobotics.tujiaovision.$Profile-tests"
    [pscustomobject]@{
        Root = $root; Output = $output; Target = $target; Identifier = $identifier
        Config = Join-Path $output 'tauri-test-config.json'
        Executable = Join-Path $output 'gluesight-ui-tests.exe'
        BuildManifest = Join-Path $output 'build.json'
        Manifest = Join-Path $output 'instance.json'
        Overlay = [ordered]@{
            productName = $title; identifier = $identifier
            build = [ordered]@{ beforeBuildCommand = 'node node_modules/typescript/bin/tsc -b && node node_modules/vite/bin/vite.js build' }
            app = [ordered]@{ windows = @([ordered]@{ label = 'main'; title = $title; visible = $false; width = 1440; height = 900; minWidth = 1100; minHeight = 700 }) }
            bundle = [ordered]@{ active = $false }
        }
    }
}

function Enter-UiTestLock {
    param([Parameter(Mandatory)][string]$Path)
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path))
    try { [IO.File]::Open($Path, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
    catch [IO.IOException] { throw "桌面测试或构建正在使用此资源，拒绝并行执行：$Path" }
}

function Assert-UiTestBuild {
    param([Parameter(Mandatory)]$Layout)
    if (!(Test-Path -LiteralPath $Layout.Executable -PathType Leaf)) { throw 'SkipBuild 需要已有的隔离测试可执行文件。' }
    $metadata = if (Test-Path -LiteralPath $Layout.BuildManifest -PathType Leaf) { $Layout.BuildManifest } else { $Layout.Manifest }
    if (!(Test-Path -LiteralPath $metadata -PathType Leaf)) { throw '隔离构建清单不存在，请重新构建。' }
    $previous = Get-Content -LiteralPath $metadata -Raw | ConvertFrom-Json
    if ($previous.identifier -ne $Layout.Identifier -or $previous.executable -ne $Layout.Executable) {
        throw '隔离测试实例标识或路径不匹配，请重新构建。'
    }
}
