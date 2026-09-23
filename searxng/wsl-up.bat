@echo off
REM Daily helper: start Docker daemon + SearXNG stack inside WSL2. No admin needed.
for /f "delims=" %%i in ('wsl -d Ubuntu -u root -e wslpath -a "%~dp0wsl-setup.sh"') do set SH_UNIX=%%i
wsl -d Ubuntu -u root -e bash "%SH_UNIX%" --up
