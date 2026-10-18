param(
  [string]$SampleSet = $env:GLUESIGHT_SAMPLE_SET,
  [string]$CoreDll = $env:LYFLOW_CORE_DLL,
  [string]$Output,
  [string]$Stages,
  [string]$Detect,
  [string]$Graph,
  [string]$Fixture
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (!$SampleSet -or !(Test-Path -LiteralPath (Join-Path $SampleSet 'sample-set.json') -PathType Leaf)) { throw '需要样本集目录（-SampleSet 或 GLUESIGHT_SAMPLE_SET）' }
if (!$CoreDll -or !(Test-Path -LiteralPath $CoreDll -PathType Leaf)) { throw '需要 lyflow_core.dll 路径（-CoreDll 或 LYFLOW_CORE_DLL）' }
if (!$Output) { $Output = Join-Path $root ('output/algorithm-eval/' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
$out = [IO.Path]::GetFullPath($Output, (Get-Location).Path)
$vars = [ordered]@{
  GLUESIGHT_SAMPLE_SET = (Resolve-Path -LiteralPath $SampleSet).Path
  LYFLOW_CORE_DLL = (Resolve-Path -LiteralPath $CoreDll).Path
  GLUESIGHT_EVAL_OUT = $out
  GLUESIGHT_EVAL_STAGES = $Stages
  GLUESIGHT_EVAL_DETECT = $Detect
  GLUESIGHT_EVAL_GRAPH = $Graph
  GLUESIGHT_EVAL_FIXTURE = if ($Fixture) { (Resolve-Path -LiteralPath $Fixture).Path }
}
$saved = @{}
foreach ($name in $vars.Keys) { $saved[$name] = [Environment]::GetEnvironmentVariable($name) }
Push-Location -LiteralPath (Join-Path $root 'src-tauri')
try {
  foreach ($name in $vars.Keys) { if ($vars[$name]) { [Environment]::SetEnvironmentVariable($name, $vars[$name]) } }
  cargo test --lib vision::eval::algorithm_eval -- --ignored --exact --nocapture
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Pop-Location
  foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
}
Write-Host "评测结果：$out"
