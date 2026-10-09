@echo off
pwsh.exe -NoProfile -File "%~dp0Start-Demo.ps1" -OpenConsole %*
if errorlevel 1 (
  pause
  exit /b 1
)
