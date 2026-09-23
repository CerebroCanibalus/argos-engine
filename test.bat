@echo off
REM Argos Engine - fast tests
set VSCMD_START_DIR=%CD%
set "VSCMD=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\Tools\VsDevCmd.bat"
if exist "%VSCMD%" (
    call "%VSCMD%" -arch=x64 > nul
)
cargo test
