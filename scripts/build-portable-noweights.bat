@echo off
rem Portable executables WITHOUT the DAC codec: much smaller, but DAC services are
rem then named, not decoded. Writes exe\decdrm-gui.exe and exe\decdrm.exe, with the C
rem runtime linked statically. See scripts\build-portable.ps1.
rem
rem   scripts\build-portable-noweights.bat
rem
rem This needs no weights at all. For the full build with the codec inside, run
rem scripts\build-portable.bat instead.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-portable.ps1" -NoWeights %*
if errorlevel 1 exit /b %errorlevel%
