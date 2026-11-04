# DecDRM — notes for Claude

Rust DRM30 (Digital Radio Mondiale, modes A–D) receiver + transmitter. Read
`docs/DESIGN.md` first: it records the user's decisions, the architecture and the
milestone checklist (keep it current).

## Layout
- `crates/decdrm-core` — pure-Rust PHY/FEC/multiplex: `params`, `tables`, `cellmap`,
  `fec/` (CRC, dispersal, interleavers, conv code, Viterbi, QAM metrics, MLC),
  `interleave` (MSC cell interleaver), `fac`, `rx/` (receiver chain; `rx::diversity`
  combines two receivers' MSC cells), `tx/`, `channel/` (annex B HF and COST 207 VHF
  models, at any working rate),
  `mux/` (SDC, service info, MSC demux, audio super frames, text messages), `dsp/`.
- `crates/decdrm-codecs` (+ `decdrm-fdk-sys`, `decdrm-opus-sys`, `decdrm-xaac-sys`) —
  FDK-AAC / libopus / libxaac (xHE-AAC encoder, patched at build time) FFI.
- `crates/decdrm-io` — WAV/FLAC, resampling, sound card (cpal), drift-compensated player,
  SBR band smoother (`sbr_smooth`, the session's *Smooth SBR* option).
- `crates/decdrm-data` — packet mode, MOT, Journaline, EPG, BWS, TPEG.
- `crates/decdrm-engine` — worker thread: source → `Session` (receiver + multiplex +
  audio/text/data pipelines) → audio out / data store; `Snapshot`s for the UIs.
- `crates/decdrm-station` — transmitter application layer (TOML station config);
  `feeds` turns RSS/Atom feeds into Journaline pages (fetched on a schedule).
  Mode E (DRM+): logical frames are 100 ms and the **audio super frame is 200 ms over two
  of them** (§5.3.1.0, `AudioPlan::parts_per_super_frame`) for AAC/HE-AAC, xHE-AAC and
  Opus; **DAC's super frame is 400 ms by construction, so it spans four** (`super_frames_for`;
  the deframer finds its phase from the DAC framing CRCs, `set_whole_check`). AAC runs at
  24/48 kHz there and the MSC uses 16- or 4-QAM only (a 64-QAM setting maps to 16-QAM; the
  SDC is 4-QAM).
  Receiver of the station's own signal: `Session` (from decdrm-engine). Mode E is found
  unpinned too: above 48 kHz `rx::freqacq` matches the 95 kHz spectrum plateau (its two
  edges put the carrier within a few Hz) and `rx::Receiver::track_cp_frequency` refines it
  from the guard interval, so `ReceiverConfig::pinned` is a shortcut, not a requirement.
  **External xHE-AAC encoder** (optional, `codec = "xhe-aac-external"`): the 32-bit helper
  `exe\xhe\decdrm-xhe-host.exe` (`tools\xhe-host`, around Fraunhofer's DRM xHE-AAC DLL,
  never committed) does the encoding in its own process and returns whole super frames plus
  the SDC type-9 body; `decdrm_codecs::xhe_external` finds it beside the executable. Without
  the folder the codec is not offered. Future options are named `xhe_ext_*`.
- `crates/decdrm-schedule` — DRM broadcast schedules (Dream's DRMDX `DRMSchedule.ini`,
  EiBi CSV): what is on the air now; GUI Schedule tab, `decdrm schedule`. Downloads only
  on the user's request (curl/wget); `src/eibi_tables.rs` is generated from Dream.
- `crates/decdrm-evs` — EVS (KCBS on 6140 kHz sends it as data): the signalling table
  and the KCBS framing (`kcbs`), for recognition only. The optional 3GPP decoder was
  removed (2026-10-01) so the repo can be public: never add 3GPP/EVS reference code
  (copyright, patent-licensed codec).
- `crates/decdrm-mdi` — MDI (TS 102 820), RSCI/RCI (TS 102 349) over DCP (TS 102 821):
  AF/PFT (Reed–Solomon `rs`), TAG items, `MdiFrame`, `RsciStatus`, `RciCommand`,
  recordings (`.rsX`, pcap/pcapng, raw), UDP in Dream's address syntax, `MdiInput`.
  **DRM+ is marked by the FAC's width** (`MdiFac`: 72 bits for A–D, 116 for mode E; `robm`
  names A–D only). Engine input `InputSpec::Mdi` (`Session::push_mdi`), `EngineConfig::rci_listen`;
  the station's modulator (`[mdi]`, `modulator.rs`) and `Station::capture_mdi`
  (`--example mdi_source` makes test MDI). No MDI output role (not asked for).
- `crates/decdrm-kiwi` — KiwiSDR client (WebSocket protocol after kiwiclient, typed
  path `/no_wf/<ts>/SND`, own upgrade handshake for the proxy's HTTP/1.0 redirects,
  tungstenite framing), `KiwiStream` (thread + I/Q FIFO; `tune` retunes on the open
  connection, `--example tune` checks that live), the public Kiwi list parser,
  and `mock::MockKiwi` (a stand-in Kiwi on 127.0.0.1 for tests). Engine input
  `InputSpec::Kiwi`; CLI `decdrm rx --kiwi`; GUI *KiwiSDR* source and *Find…* window.
  Live tests on public Kiwis only when the user agrees; never pretend to be a browser.
- `crates/decdrm-dac` — DAC (Descript Audio Codec, 24 kHz), DecDRM's neural codec
  extension (it replaced EnCodec on 2026-10-02; old `ENC1` services are named, not
  decoded); candle only with the `dac` feature (engine/station/cli/gui forward it).
  Streaming model with exact chunking (`model.rs`, tested against `model/reference.rs`).
  Mono or joint stereo (mid + side, each its own DAC stream; `DacConfig::side`).
  Weights live in the git-ignored `models/` (`decdrm models download dac`).
- `apps/decdrm-cli` (binary `decdrm`: `rx` (also `--kiwi`, `--kiwi2` for diversity), `tx`, `schedule`, `devices`, `models`), `apps/decdrm-gui` (egui,
  MSRV 1.95 because of eframe).
- `third_party/` — pinned submodules (fdk-aac v2.0.3, opus v1.6.1, libxaac v0.1.13);
  clone with `--recursive`.
- `samples/` (user recordings) and `reference/` (Dream r1548 source, ETSI PDFs) are
  git-ignored and never committed. `reference/etsi/es_201980v040201p.txt` is the DRM
  spec as text — grep it for clause numbers and tables.

## Commands
- Build/test: `cargo test -p decdrm-core` (DSP is slow unoptimised; the dev profile
  uses opt-level 1, deps at 3).
- Full decode of a recording (audio, text, data):
  `cargo run --release -p decdrm-cli -- rx samples/DW_ModeB_10kHz.flac [--out a.wav]
  [--data-dir DIR] [--format iq]`; GUI: `cargo run --release -p decdrm-gui -- FILE`.
  The working rate follows the file's own rate (`WorkingRate::closest`); `--sample-rate`
  overrides it, and a DRM+ file needs no mode option (see the mode E note above).
- Receiver-only smoke test:
  `cargo run --release -p decdrm-core --example rxfile -- samples/DW_ModeB_10kHz.flac`
  (append `iq` for I/Q files). `--example spectrum` prints a coarse spectrum,
  `--example modescores` the mode-detection margins per layout/SNR.
- Loopback suite incl. long sweeps:
  `cargo test --release -p decdrm-core --test loopback -- --include-ignored --nocapture`.
- Testing runs on GitHub Actions (the user's choice, 2026-10-03): `ci.yml` on every push
  to main and every PR — Linux and Windows build, all tests incl. the long loopback and
  diversity sweeps, clippy, `scripts/smoke-test.sh`; a Linux job runs the DAC tests with
  the real weights. Push, then `gh run watch` / `gh run list`; rerun by hand with
  `gh workflow run ci.yml`. Locally only what a change needs (and the recordings in
  `samples/`, which CI does not have).
- Releases: push the version-bump commit's tag `vX.Y.Z`; `release.yml` builds the
  portable Windows exes, checks and smoke-tests them and uploads them to a draft
  release. Then write the notes and publish:
  `gh release edit vX.Y.Z --title … --notes-file … --draft=false --latest`.
- Portable Windows executables locally: `scripts\build-portable.bat` (with DAC and its
  weights, ~300 MB; needs `decdrm models download dac`) and
  `scripts\build-portable-noweights.bat` (without the codec, much smaller), both wrappers
  around `build-portable.ps1` (`-NoWeights` is the flag behind the second). Writes
  `exe\decdrm-gui.exe` and `exe\decdrm.exe`. The optional external xHE-AAC encoder's
  32-bit helper is built by `scripts\build-xhe-host.bat` into `exe\xhe\` (with the
  Fraunhofer DLL when it is in `refs\dll`); see `tools\xhe-host\README.md`.

## Conventions
- DSP in `f64` (`Real`, `Cplx`); PCM audio is `f32`. Working sample rate 48 kHz.
- Bit streams are `Vec<u8>` with one bit per byte (`bits::BitReader/BitWriter`).
- Carrier index k ↔ FFT bin k mod N (true baseband, DC carrier at 0 Hz); carrier
  offset `c = k − Kmin` indexes per-symbol arrays.
- Timing shifts: `SymbolWindow::shift` > 0 means the window moved later; older channel
  values are rotated by e^{+j2πkΔ/N} to the newer timing. Dream's `iCurTimeCorr` is −shift.
- Cite ES 201 980 clauses and the Dream file an algorithm came from in doc comments.
- User is strong in DSP, newer to Rust: explain non-obvious Rust idioms briefly.
- Git: commit per milestone, repo-local identity already set (GitHub no-reply address);
  push to `origin` (github.com/CasualArclamp/DecDRM). The repo is public since
  2026-10-01: never commit recordings, reference material, weights, secrets or 3GPP code.
- Sub-agents share one target dir and a 31 GB machine: one cargo command at a time, no
  load generators, sound-card tests `#[ignore]`d, never play audio audibly.
