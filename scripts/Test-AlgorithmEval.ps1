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
$env:GLUESIGHT_SAMPLE_SET = (Resolve-Path -LiteralPath $SampleSet).Path
$env:LYFLOW_CORE_DLL = (Resolve-Path -LiteralPath $CoreDll).Path
$env:GLUESIGHT_EVAL_OUT = [IO.Path]::GetFullPath($Output, (Get-Location).Path)
if ($Stages) { $env:GLUESIGHT_EVAL_STAGES = $Stages }
if ($Detect) { $env:GLUESIGHT_EVAL_DETECT = $Detect }
if ($Graph) { $env:GLUESIGHT_EVAL_GRAPH = $Graph }
if ($Fixture) { $env:GLUESIGHT_EVAL_FIXTURE = (Resolve-Path -LiteralPath $Fixture).Path }
Push-Location -LiteralPath (Join-Path $root 'src-tauri')
try {
  cargo test --lib vision::eval::algorithm_eval -- --ignored --exact --nocapture
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally { Pop-Location }
Write-Host "评测结果：$env:GLUESIGHT_EVAL_OUT"
