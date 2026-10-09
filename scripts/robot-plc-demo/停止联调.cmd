@echo off
pwsh.exe -NoProfile -File "%~dp0Stop-Demo.ps1" %*
if errorlevel 1 pause
