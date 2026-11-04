# decdrm-xhe-host

A 32-bit bridge to Fraunhofer IIS's DRM xHE-AAC / HE-AAC encoder DLL (`fhgio.dll` or
`fhgxaac.dll` — the library shipped with the *Spark Modulator* DRM+ transmitter). DecDRM's
own executables are 64-bit and cannot load it, so this small host process does the encoding
and talks to DecDRM over stdio.

**It is not built by the workspace and not shipped.** The DLL is a commercial Fraunhofer
encoder under its embedded licence blob; build this locally, put `fhgio.dll` beside it in
`exe\xhe\` next to the portable executables, and keep both out of the repository (the repo
is public). DecDRM looks for `xhe\decdrm-xhe-host.exe` beside itself: with the folder
absent (or the tool missing from it) the external encoder options are simply not offered.

## Build (32-bit MSVC)

```bat
scripts\build-xhe-host.bat
```

which is the same as

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat" x86
cl /O2 /nologo /W3 host.c /Fe:../exe/xhe/decdrm-xhe-host.exe
```

The script also copies `refs\dll\fhgio.dll` into `exe\xhe\` when it is there. The DLL
needs the Visual C++ 2010 runtime (`MSVCR100.dll`, x86), which Spark Modulator installs —
without it the DLL will not load.

## Usage

```text
decdrm-xhe-host [--dll PATH] [--drmplus|--drm30] [--channels 1|2] --bytes N
                [--probe | --selftest [--seconds N]]
```

`--bytes` is the size of one DRM audio super frame — the station knows it from its
multiplex plan. The bitrate is derived from it: `bytes · 8 / T`, with `T = 0.2 s` in
DRM+ and `T = 0.4 s` in DRM30.

* `--probe` opens the encoder, prints the header line below, closes, and exits. A smoke
  test for the DLL and its parameters.
* `--selftest` feeds a 1 kHz sine and checks every super frame has the expected size.
* Otherwise it runs the protocol.

## Protocol (binary stdio, one line per message)

The host always starts with:

```text
OK <frameBytes> <nSmpFloats> <pcmRate> <channels> <drmPlus> <sdcHex>
```

`nSmpFloats` is how many interleaved `f32` samples the encoder wants per call (not per
super frame — it takes a few calls to produce one), `pcmRate` is 48000, and `sdcHex` is
the 5–8 byte SDC fragment, which is the SDC type-9 body (2 header bytes plus the xHE-AAC
static config). DecDRM parses it with `AudioInfo::from_type9_bytes`.

Then, repeatedly:

```text
NEED <n>       the host reads n f32 samples (little-endian) from stdin
FRAME <len>    followed by len bytes of single super frame (plain, not XORed)
ERROR <code> <text>   the encoder failed; the host exits
```

The JNI wrappers in the DLL XOR the output with a key; calling the internal entry points
directly (as here) returns plain bytes, so no de-obfuscation is needed.

## Parameters and limits (from the DLL itself)

* channels 1 or 2; bitrate 6000–163920 bit/s; stereo needs ≥ 12000; mono ≤ 39999;
  DRM30 needs ≤ 71960 (DRM+ up to 96 kbit/s verified).
* One encoder instance per process; close exactly once (closing an invalid handle crashes).
* Not thread-safe: one process, one stream.
