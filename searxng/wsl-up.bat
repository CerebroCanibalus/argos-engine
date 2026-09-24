@echo off
setlocal enabledelayedexpansion
REM Daily helper: start Docker daemon + SearXNG stack inside WSL2. No admin needed.
REM Uses the absolute WSL path so a stale PATH cannot pick the in-box wsl.exe.

set "WSL_EXE=%ProgramFiles%\WSL\wsl.exe"
if not exist "!WSL_EXE!" set "WSL_EXE=%SystemRoot%\System32\wsl.exe"

set "WINPATH=%~dp0wsl-setup.sh"
set "DRIVE=%~d0"
set "DRIVE1=!DRIVE:~0,1!"
set "DRIVE_LC="
for %%c in (a b c d e f g h i j k l m n o p q r s t u v w x y z) do if /i "%%c"=="!DRIVE1!" set "DRIVE_LC=%%c"
if "!DRIVE_LC!"=="" (
    echo [ERROR] Cannot resolve drive of %~dp0
    exit /b 1
)
set "REST=!WINPATH:~2!"
set "SH_UNIX=/mnt/!DRIVE_LC!!REST:\=/"

"!WSL_EXE!" -d Ubuntu -u root -e bash "!SH_UNIX!" --up
