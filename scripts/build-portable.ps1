# Builds the two single-file DecDRM executables for 64-bit Windows 10/11 that need
# nothing installed: the C runtime is linked statically (so no Visual C++
# redistributable is needed) and, unless -NoWeights is given, the neural codec DAC
# comes with its weights built in. The results go to exe\ (git-ignored):
#
#   decdrm-gui.exe   the desktop GUI (receiver + transmitter)
#   decdrm.exe       the command line
#
# Two variations of this build, one flag apart:
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-portable.ps1
#       WITH the DAC codec and its weights built in (~300 MB per executable).
#       Needs the weights at build time: `decdrm models download dac`, or point
#       $env:DECDRM_MODELS at an existing models directory.
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-portable.ps1 -NoWeights
#       WITHOUT the DAC codec: much smaller, but DAC services are then named, not
#       decoded. No weights needed.
#
# Or run the wrappers scripts\build-portable.bat and
# scripts\build-portable-noweights.bat, which pick one of the two for you.
#
# Builds in target\portable, so the usual target directory and its cache stay as they
# are.
param([switch]$NoWeights)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$triple = "x86_64-pc-windows-msvc"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found on PATH; install Rust (https://rustup.rs) first"
}

# The weights are 285 MiB; `embed-weights` is what actually includes them (see
# crates/decdrm-dac/build.rs). Check here so a missing download is a one-line message
# instead of a build-script panic several minutes in.
if (-not $NoWeights) {
    $models = if ($env:DECDRM_MODELS) { $env:DECDRM_MODELS } else { Join-Path $root "models" }
    $weights = Join-Path $models "dac_24khz\model.safetensors"
    if (-not (Test-Path $weights)) {
        throw "the DAC weights are not at $weights; run 'decdrm models download dac' first, set DECDRM_MODELS, or build with -NoWeights"
    }
}

# With an explicit --target the flags reach only the executables, not build scripts
# and proc macros.
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$env:CARGO_TARGET_DIR = Join-Path $root "target\portable"
# Two 300 MB links at once want more memory than a smaller machine has to spare.
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = "6" }
# The release profile carries line tables, and MSVC then embeds the compile command
# line in every object (/Z7). With the codec crates' long include list and a deep
# workspace path that fails (D8050: c1xx cannot write the debug record), so portable
# binaries are built without debug info -- which they do not need.
$env:CARGO_PROFILE_RELEASE_DEBUG = "0"
# MSVC's librarian and linker scratch files go to %TEMP%, which a sandbox, a cleaner
# or a scanner can lock (lib.exe then fails with LNK1104 on lnk{GUID}.tmp). Keep them
# in the workspace instead.
$env:TEMP = Join-Path $root "target\tmp\buildtmp"
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null

Push-Location $root
try {
    if ($NoWeights) {
        Write-Host "Building WITHOUT the DAC codec (small executables)..."
        cargo build --release --target $triple -p decdrm-gui -p decdrm-cli
    } else {
        Write-Host "Building WITH the DAC codec and its weights (large executables)..."
        cargo build --release --target $triple -p decdrm-gui -p decdrm-cli `
            --features decdrm-gui/embed-weights,decdrm-cli/embed-weights
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

    $out = Join-Path $root "exe"
    New-Item -ItemType Directory -Force $out | Out-Null
    foreach ($exe in "decdrm-gui.exe", "decdrm.exe") {
        Copy-Item (Join-Path $env:CARGO_TARGET_DIR "$triple\release\$exe") $out -Force
        $mb = (Get-Item (Join-Path $out $exe)).Length / 1MB
        Write-Host ("  {0,-16} {1,7:N1} MB" -f $exe, $mb)
    }
    Write-Host "written to $out"
    if ($NoWeights) {
        Write-Host "built WITHOUT the DAC codec: DAC services are named, not decoded"
    } else {
        Write-Host "built WITH the DAC codec: its weights are inside the executables"
    }
} finally {
    Pop-Location
}
