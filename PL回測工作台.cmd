@echo off
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0pl-workbench.ps1"
if errorlevel 1 pause
if errorlevel 1 pause
