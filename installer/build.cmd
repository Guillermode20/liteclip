@echo off
setlocal enabledelayedexpansion

:: Delegate to the PowerShell build script so installer + portable outputs stay in sync
powershell -ExecutionPolicy Bypass -File "%~dp0build.ps1"
if errorlevel 1 goto :err

echo Build complete. Outputs located in installer\output
exit /b 0

:err
echo Build failed.
exit /b 1
