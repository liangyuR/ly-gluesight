#Requires -Version 7.0
param([Parameter(Mandatory)][string]$Archive)
. (Join-Path $PSScriptRoot 'Vendor-Dependency.ps1')
Assert-RuntimePlatform
$config = Get-RuntimeConfiguration
$Archive = (Resolve-Path -LiteralPath $Archive).Path
if ([IO.Path]::GetFileName($Archive) -cne $config.asset) { throw "资产名必须为 $($config.asset)" }
Get-Command gh -ErrorAction Stop | Out-Null
$stageRoot = Join-Path $PSScriptRoot '../src-tauri/target/publish-runtime'
Reset-RuntimeStage $stageRoot @($Archive)
Expand-RuntimeArchive $Archive $stageRoot
[void](Assert-VendorRuntime (Join-Path $stageRoot 'runtime'))
$existing = & gh api "repos/$($config.repository)/releases/tags/$($config.tag)" 2>&1
if ($LASTEXITCODE -eq 0) { throw '依赖 Release 已存在；请使用新依赖版本，禁止覆盖既有资产。' }
if (($existing -join "`n") -notmatch '(HTTP 404|Not Found)') { throw "无法确认依赖 Release 是否存在：$($existing -join ' ')" }
& gh release create $config.tag $Archive --repo $config.repository --prerelease --title "Windows x64 厂商运行库 v$($config.bundleVersion)" --notes 'GlueSight 固定版本厂商依赖：MVS、许可证和 VC++ CRT。lyFlow 运行库从 lyFlow 仓库 Release 独立恢复。现场需预装 MVS 驱动及 WebView2。'
if ($LASTEXITCODE -ne 0) { throw "发布依赖 Release 失败（退出码 $LASTEXITCODE）；禁止用覆盖上传重试。" }
