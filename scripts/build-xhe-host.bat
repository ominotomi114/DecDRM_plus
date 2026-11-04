@echo off
rem Builds the 32-bit bridge to Fraunhofer's DRM xHE-AAC encoder DLL (fhgio.dll) into
rem exe\xhe\decdrm-xhe-host.exe, beside the portable executables. DecDRM's own binaries
rem are 64-bit, so this side tool does the encoding in its own process and talks
rem PCM/frames over stdio. See tools\xhe-host\README.md.
rem
rem The DLL is commercial and is NOT shipped: exe\xhe\ must also hold fhgio.dll (copied
rem here from refs\dll if it is present). Without the folder, or without the tool in it,
rem DecDRM simply does not offer the external encoders.
rem
rem fhgio.dll needs the Visual C++ 2010 runtime (x86), which Spark Modulator installs.
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

if not exist "%ROOT%\exe\xhe" mkdir "%ROOT%\exe\xhe"
if not exist "%ROOT%\target\tmp" mkdir "%ROOT%\target\tmp"

pushd "%ROOT%\tools\xhe-host"
cl /O2 /nologo /W3 host.c /Fo:"%ROOT%\target\tmp\xhe-host.obj" /Fe:"%ROOT%\exe\xhe\decdrm-xhe-host.exe"
set RC=%errorlevel%
popd
if %RC% neq 0 exit /b %RC%

rem A local copy of the DLL for testing: refs\ is git-ignored, like exe\.
if not exist "%ROOT%\exe\xhe\fhgio.dll" if exist "%ROOT%\refs\dll\fhgio.dll" (
    copy /y "%ROOT%\refs\dll\fhgio.dll" "%ROOT%\exe\xhe\fhgio.dll" >nul
    echo copied fhgio.dll from refs\dll
)

if exist "%ROOT%\exe\xhe\fhgio.dll" (
    echo written exe\xhe\decdrm-xhe-host.exe ^(the DLL is beside it^)
) else (
    echo written exe\xhe\decdrm-xhe-host.exe
    echo put fhgio.dll ^(from Spark Modulator's lib32^) beside it to use it
)
