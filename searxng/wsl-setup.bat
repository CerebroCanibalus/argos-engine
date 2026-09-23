@echo off
REM One-time provisioning: WSL2 + Ubuntu + Docker Engine + SearXNG stack.
REM Use when Docker Desktop is unavailable - Windows builds below 19045 (e.g. LTSC 21H2).
REM Run normally: UAC will pop, approve it. Requires a prior reboot after WSL features were enabled.

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo Requesting administrator privileges - UAC prompt incoming...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

echo [1/3] Checking Ubuntu distro...
wsl -d Ubuntu -e true > nul 2>&1
if errorlevel 1 (
    echo Installing Ubuntu distro - this downloads several hundred MB...
    wsl --install -d Ubuntu --no-launch
)

echo [2/3] First boot of the distro...
wsl -d Ubuntu -u root -e true

for /f "delims=" %%i in ('wsl -d Ubuntu -u root -e wslpath -a "%~dp0wsl-setup.sh"') do set SH_UNIX=%%i

echo [3/3] Provisioning Docker Engine + SearXNG...
wsl -d Ubuntu -u root -e bash "%SH_UNIX%"
set RERR=%errorlevel%
echo.
echo Result code: %RERR%
echo Daily start from now on: searxng\wsl-up.bat - no admin needed.
pause
