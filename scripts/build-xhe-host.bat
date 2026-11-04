@echo off
rem Builds the 32-bit bridge to Fraunhofer's DRM xHE-AAC encoder DLL (fhgio.dll):
rem exe\decdrm-xhe-host.exe. DecDRM's own executables are 64-bit, so this side tool does
rem the encoding in its own process and talks PCM/frames over stdio. See
rem tools\xhe-host\README.md.
rem
rem The DLL is commercial and is NOT shipped: put fhgio.dll beside the host in exe\.
rem It needs the Visual C++ 2010 runtime (x86), which Spark Modulator installs.
setlocal
set ROOT=%~dp0..
set VCVARS="%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if not exist %VCVARS% set VCVARS="%ProgramFiles%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if not exist %VCVARS% (
    echo Could not find vcvarsall.bat for Visual Studio 2022 Build Tools.
    exit /b 1
)

call %VCVARS% x86 >nul
if errorlevel 1 exit /b %errorlevel%

if not exist "%ROOT%\exe" mkdir "%ROOT%\exe"
if not exist "%ROOT%\target\tmp" mkdir "%ROOT%\target\tmp"

pushd "%ROOT%\tools\xhe-host"
cl /O2 /nologo /W3 host.c /Fo:"%ROOT%\target\tmp\xhe-host.obj" /Fe:"%ROOT%\exe\decdrm-xhe-host.exe"
set RC=%errorlevel%
popd

if %RC% neq 0 exit /b %RC%
if exist "%ROOT%\exe\fhgio.dll" (
    echo written exe\decdrm-xhe-host.exe ^(fhgio.dll is beside it^)
) else (
    echo written exe\decdrm-xhe-host.exe
    echo put fhgio.dll ^(from Spark Modulator's lib32^) beside it to use it
)
