@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0pl-workbench-godot.ps1" -OpenTradeCom
if errorlevel 1 pause
