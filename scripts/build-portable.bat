@echo off
rem Portable executables WITH the DAC codec and its weights built in (about 300 MB
rem each): exe\decdrm-gui.exe and exe\decdrm.exe. Static C runtime, nothing to install.
rem See scripts\build-portable.ps1.
rem
rem   scripts\build-portable.bat
rem
rem Needs the DAC weights at build time:
rem
rem   decdrm models download dac
rem
rem or point DECDRM_MODELS at an existing models directory. For the small build
rem without the codec, run scripts\build-portable-noweights.bat instead.
rem
rem Any extra arguments are passed on, so `build-portable.bat -NoWeights` works too.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-portable.ps1" %*
if errorlevel 1 exit /b %errorlevel%
