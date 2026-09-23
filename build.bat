@echo off
REM Argos Engine - build + test (Windows)
set VSCMD_START_DIR=%CD%
set "VSCMD=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\Tools\VsDevCmd.bat"
if exist "%VSCMD%" (
    call "%VSCMD%" -arch=x64 > nul
) else (
    echo [INFO] VsDevCmd not found; relying on cargo MSVC auto-detection
)

if not exist "Cargo.toml" (
    echo [ERROR] Cargo.toml not found. Run this script from the repo root.
    exit /b 1
)

REM Kill any running build output before compiling (file lock prevention)
taskkill /F /IM argos-engine.exe > nul 2>&1

echo [1/3] cargo build --release ...
cargo build --release
if errorlevel 1 (
    echo [ERROR] build failed
    exit /b 1
)

echo [2/3] cargo test ...
cargo test
if errorlevel 1 (
    echo [ERROR] tests failed
    exit /b 1
)

echo [3/3] OK: target\release\argos-engine.exe
