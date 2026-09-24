@echo off
setlocal enabledelayedexpansion
REM One-time provisioning: modern WSL2 + Ubuntu + Docker Engine + SearXNG stack.
REM Use when Docker Desktop is unavailable - Windows builds below 19045 (e.g. LTSC 21H2).
REM Kernel: in-box "wsl --update" is a no-op on this LTSC box; step1 installs the official
REM modern WSL MSI from GitHub releases - it BUNDLES the kernel (verified: 6.18.33.2-2).
REM PATH note: always call WSL_EXE by absolute path - the stale System32 in-box wsl.exe
REM shadows the new binary on old PATHs (missing options like --no-launch).
REM Error handling note: wsl.exe returns -1 on failures; use "!errorlevel! neq 0",
REM never "if errorlevel 1" (signed comparison would skip -1).

net session >nul 2>&1
if not "!errorlevel!"=="0" (
    echo Requesting administrator privileges - UAC prompt incoming...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

set "WSL_EXE=%ProgramFiles%\WSL\wsl.exe"
if not exist "!WSL_EXE!" set "WSL_EXE=%SystemRoot%\System32\wsl.exe"

echo [1/4] Installing modern WSL2 - the MSI bundles the kernel...
set "MSI="
for %%I in ("%~dp0..\.temp\wsl.msi") do set "MSI=%%~fI"
if not exist "!MSI!" (
    echo [ERROR] Missing installer: !MSI!
    echo         Download wsl.*.x64.msi from https://github.com/microsoft/WSL/releases
    pause
    exit /b 1
)
msiexec /i "!MSI!" /qn /norestart
set ERR=!errorlevel!
if !ERR! equ 0 goto msi_ok
if !ERR! equ 3010 goto msi_ok
echo [ERROR] WSL MSI failed with exit !ERR! - read any MSI message above.
pause
exit /b 1
:msi_ok
echo WSL MSI installed - exit code !ERR! - using: !WSL_EXE!

echo [2/4] Installing Ubuntu distro if missing...
"!WSL_EXE!" -d Ubuntu -e true > nul 2>&1
set ERR=!errorlevel!
if !ERR! neq 0 (
    echo Distro not present yet - downloading now, several hundred MB...
    "!WSL_EXE!" --install -d Ubuntu --no-launch
    set ERR=!errorlevel!
    if !ERR! neq 0 (
        echo [ERROR] Distro install failed with exit !ERR! - read any message above.
        echo [HINT] If the message mentions kernel or features, enable Developer mode:
        echo         Settings - Privacy ^& security - For developers - Developer mode ON.
        pause
        exit /b 1
    )
)

echo [3/4] First boot of the distro - the real kernel test...
"!WSL_EXE!" -d Ubuntu -u root -e true
set ERR=!errorlevel!
if !ERR! neq 0 (
    echo [ERROR] First boot failed with exit !ERR!.
    echo [HINT] Kernel problems should be gone after step1; check the output above.
    pause
    exit /b 1
)

REM Convert %~dp0wsl-setup.sh to its WSL path: lowercase drive + slash substitution.
REM Snippet validated against a real run (spaces + drive letter + backslashes).
set "WINPATH=%~dp0wsl-setup.sh"
set "DRIVE=%~d0"
set "DRIVE1=!DRIVE:~0,1!"
set "DRIVE_LC="
for %%c in (a b c d e f g h i j k l m n o p q r s t u v w x y z) do if /i "%%c"=="!DRIVE1!" set "DRIVE_LC=%%c"
if "!DRIVE_LC!"=="" (
    echo [ERROR] Cannot resolve drive of %~dp0
    pause
    exit /b 1
)
set "REST=!WINPATH:~2!"
set "SH_UNIX=/mnt/!DRIVE_LC!!REST:\=/"
if /i not "!SH_UNIX:~0,5!"=="/mnt/" (
    echo [ERROR] WSL path conversion produced: !SH_UNIX!
    pause
    exit /b 1
)
if not exist "!WINPATH!" (
    echo [ERROR] Script not found: !WINPATH!
    pause
    exit /b 1
)

echo [4/4] Provisioning Docker Engine + SearXNG - script: !SH_UNIX!
"!WSL_EXE!" -d Ubuntu -u root -e bash "!SH_UNIX!"
set RERR=!errorlevel!
echo.
echo Result code: !RERR!
if "!RERR!"=="0" (
    echo Provisioning complete - the JSON API is answering on 127.0.0.1:8080.
    echo From now on the MCP boots the stack on demand; manual helper: searxng\wsl-up.bat
) else (
    echo [ERROR] Provisioning failed with exit !RERR! - check the output above.
)
pause
