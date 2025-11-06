@echo off
REM Helper script to run the PowerShell build script
REM This ensures the PowerShell script runs in a visible window and doesn't close immediately

echo.
echo ================================================
echo Smart Video Compressor - Build Script Launcher
echo ================================================
echo.
echo Starting PowerShell build script...
echo.

REM Run PowerShell script with -NoExit to keep window open on error
REM Remove -NoExit after testing so the window closes properly on success
powershell.exe -ExecutionPolicy Bypass -File "%~dp0publish-win.ps1"

REM Capture exit code
set EXIT_CODE=%ERRORLEVEL%

REM If there was an error, pause before closing
if %EXIT_CODE% NEQ 0 (
    echo.
    echo ================================================
    echo Build script exited with error code: %EXIT_CODE%
    echo ================================================
    pause
)

exit /b %EXIT_CODE%

