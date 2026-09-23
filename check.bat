@echo off
REM Argos Engine - quality gates (format + lints)
set VSCMD_START_DIR=%CD%
set "VSCMD=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\Tools\VsDevCmd.bat"
if exist "%VSCMD%" (
    call "%VSCMD%" -arch=x64 > nul
)

cargo fmt --check
if errorlevel 1 (
    echo [ERROR] formatting issues found (run fmt.bat)
    exit /b 1
)

cargo clippy --all-targets -- -D warnings
if errorlevel 1 (
    echo [ERROR] clippy findings
    exit /b 1
)

echo [OK] check passed
