@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0pl-workbench-godot.ps1" -OpenMixRouteKey
if errorlevel 1 pause
