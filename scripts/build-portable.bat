@echo off
rem Builds the portable, single-file Windows executables into exe\ (decdrm-gui.exe and
rem decdrm.exe): static C runtime, nothing to install. See scripts\build-portable.ps1.
rem
rem   scripts\build-portable.bat
rem   scripts\build-portable.bat -NoWeights
rem
rem Needs the DAC weights at build time (about 299 MB, built into the executables):
rem
rem   decdrm models download dac
rem
rem or point DECDRM_MODELS at an existing models directory. With -NoWeights the
rem executables are built without the DAC codec: much smaller, but DAC services are then
rem named, not decoded.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-portable.ps1" %*
if errorlevel 1 exit /b %errorlevel%
