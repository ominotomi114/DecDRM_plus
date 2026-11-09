# DecDRM — design

DecDRM is a Digital Radio Mondiale (DRM30) receiver **and** transmitter written in Rust.
This document records the decisions from the kickoff Q&A (2026-09-29), the architecture,
and the milestone plan. Keep the milestone checklist current.

## Decisions

| Topic | Decision |
|---|---|
| Standard scope | DRM30 robustness modes **A, B, C, D**; all spectrum occupancies (0–5, 4.5–20 kHz); 4/16/64-QAM; SM, HMsym, HMmix; short & long interleaving; all protection levels. **DRM+ (mode E)** added 2026-10-23: transmit and receive both work, including reception without pinning the mode — see the milestone at the end of this document. No analog AM/SSB/FM, no AMSS. |
| Purpose | Personal / hobby. |
| Relationship to Dream | Spec-first idiomatic Rust, freely translating Dream's proven algorithms (sync, Wiener channel estimation, MLC decoding). Licence therefore **GPL-2.0-or-later**. Reference: Dream `branches/dream-mjf` r1548. |
| Receiver performance | Match or beat Dream on weak/fading signals (Wiener channel estimation, soft-decision Viterbi, iterative MLC, sample-rate-offset tracking). |
| Audio decoding | Behind a Rust codec interface. AAC / HE-AAC v1/v2 and **xHE-AAC** via **FDK-AAC** (vendored, built from source, statically linked). **Opus** (Dream's extension) via libopus. CELP/HVXC: detected and reported as unsupported. **EVS** sent as data by KCBS (6140 kHz): recognised and shown, not decoded. An optional build of the 3GPP reference decoder (feature `evs`, from a user-supplied source zip) was removed on 2026-10-01 so the repository can be public; no 3GPP code goes into DecDRM. |
| Data services | Text messages, Journaline, MOT Slideshow, EPG, Broadcast Website, TPEG/unknown (raw data saved). Broadcast clock and alternative-frequency (AFS) info from the SDC. |
| Inputs | Recorded files (WAV/FLAC; real IF or I/Q; any sample rate), live sound card (including a virtual audio cable fed by any web SDR), and KiwiSDRs directly over the network (their I/Q, retunable while connected; added 2026-10-01 at the user's request). No direct SDR drivers. The network clients are the KiwiSDR client and the transmitter's web stream audio input. |
| Outputs | Live audio (with clock-drift compensation) and logs/metrics (CSV/JSON). |
| Transmitter | Full transmitter: AAC/HE-AAC (FDK encoder), xHE-AAC (libxaac encoder), Opus, plus a neural codec (via candle) as a DecDRM-only extension: **DAC** since 2026-10-02 (EnCodec before; see the milestone *DAC replaces EnCodec*). Carries every data service the receiver decodes. Programme audio from a file, a sound card, a test tone or an internet radio stream (Icecast/SHOUTCAST over HTTP/HTTPS, its titles as text messages). Output to WAV/FLAC file and sound card. Includes a channel simulator (spec channel models) for loopback testing. |
| UI | Library crates + CLI + desktop GUI (**egui**). The GUI has RX/TX modes like Dream; the transmitter is also scriptable from the CLI with a TOML config. |
| Platforms | Windows x86_64 (primary), Linux x86_64. |
| Workflow | Max autonomy; commit per milestone; local git + GitHub repo `CasualArclamp/DecDRM`, private until 2026-10-01 and public since (so: no recordings, reference material, model weights, secrets or 3GPP code in it). |
| First milestone | Audio from a real recording, end to end, via the CLI. |

## Architecture

```
            ┌────────────── apps ──────────────┐
            │ decdrm (CLI)      decdrm-gui (egui)│
            └──────┬───────────────────┬───────┘
                   │    decdrm-engine   │   threads, device/file plumbing,
                   │ (runtime wiring)   │   codec + data-decoder registry
      ┌────────────┼──────────┬─────────┴───────┬────────────────┐
      │            │          │                 │                │
 decdrm-core   decdrm-codecs  decdrm-data    decdrm-io     (future) decdrm-xaac,
 PHY+FEC+mux   FDK-AAC, Opus  MOT/Journaline WAV/FLAC/cpal  decdrm-dac
 (pure Rust)   (FFI)          EPG/BWS/TPEG   resampling
      ▲            ▲
      │      decdrm-fdk-sys, decdrm-opus-sys (vendored C in third_party/)
```

* **decdrm-core** (pure Rust, no I/O, no C): mode/occupancy tables, OFDM cell map and
  pilots, receiver chain (input conditioning → frequency acquisition → time sync → OFDM
  demodulation → pilot-based sync → channel estimation → cell demapping → FAC/SDC/MSC
  decoding), FEC (energy dispersal, convolutional code, puncturing, soft Viterbi, bit and
  cell interleaving, MLC), FAC/SDC parsing, MSC demultiplexing, audio super-frame parsing,
  text messages, and the transmitter chain (the exact inverse) plus the channel simulator.
  DSP runs in `f64` (`Complex<f64>`); PCM audio is `f32`.
* **decdrm-codecs**: safe wrappers over FDK-AAC (DRM transport `TT_DRM`, configured with
  the raw SDC type-9 bytes, as Dream does; ADTS for web streams) and libopus.
* **decdrm-data**: packet-mode reassembly, MSC data groups, MOT (slideshow, broadcast
  website, EPG objects), Journaline, EPG binary→XML, TPEG/raw capture — decode and encode.
* **decdrm-io**: WAV/FLAC reading and writing, resampling to the 48 kHz working rate,
  sound-card input/output via cpal.
* **decdrm-engine**: glues the above together on threads (input → receiver → audio out),
  owns the codec/data registries, publishes status snapshots for the UIs, writes logs.
* **decdrm-station**: the transmitter application layer: a TOML station config
  (services, codecs, data applications, output) → planned multiplex → frames from
  `decdrm-core::tx` → WAV/FLAC or sound card. Audio inputs: file, sound card, test
  tone, web stream (`webstream`: its own HTTP/1.1 client with rustls, ICY metadata,
  playlists, MP3/ADTS framing and an Ogg demuxer; decoding by symphonia, FDK-AAC and
  libopus).
* **decdrm-schedule** (pure Rust, no DSP): broadcast schedules for "what is on the
  air now" — Dream's `DRMSchedule.ini` and EiBi's CSV, on-air logic, sources and
  downloads (curl/wget, on request only); used by `decdrm schedule` and the GUI's
  *Schedule* tab.

### Receiver data flow

The receiver is push-driven and single-threaded internally: `Receiver::push(&[samples])`
runs every stage for which enough input is buffered and emits `ReceiverEvent`s (status,
FAC/SDC updates, audio super-frames, data-stream frames, text messages). Stages keep their
own buffers; timing/frequency corrections flow backwards through explicit feedback fields
rather than shared global state (Dream's `CParameter` is split into typed per-stage state).

The working sample rate defaults to **48 kHz** (Dream's `SOUNDCRD_SAMPLE_RATE`), at which the
useful symbol lengths are 1152/1024/704/448 samples for modes A/B/C/D. It is a setting
(`WorkingRate`), because robustness mode E (DRM+) carries 213 carriers reaching ±47.11 kHz and
needs 120 or 192 kHz; the input is resampled to whatever the receiver is configured with.

## Milestones

- [x] **M0 Setup** — workspace, references, design doc, git + private GitHub.
- [x] **M1 RX spine** — file input → sync → OFDM → channel estimation → FAC/SDC/MSC →
      AAC audio (FDK) → WAV + playback, via CLI, on the mode A/B 10 kHz samples.
      *Done:* audio on every DRM recording in `samples/` (21 of 22; the 22nd is AM/AMSS).
- [x] **M2 RX completeness** — all modes A–D and occupancies 0–5, HMsym/HMmix, long
      interleaving, flipped spectrum, I/Q inputs, offsets; Wiener channel estimation,
      iterative MLC, SRO tracking; minimal modulator + channel simulator for modes/SOs we
      have no recordings of (mode D, 4.5/5/18 kHz).
      *Done:* `tests/loopback.rs` covers every layout (I/Q, real IF, offset), MSC
      bit-exact for 16/64-QAM SM, HMsym, HMmix with short/long interleaving, AWGN and
      channels 1–6, ±50 ppm / frequency offsets. Beyond Dream: sub-bin SRO estimation,
      two-window mode detection, pilot-slope SRO acquisition, seamless occupancy change.
      SRO estimation: translation of PDS snapshots (phase-slope fraction with a
      robust fallback), exact pilot-grid scaling, lag-compensated tracking;
      pilot-slope acquisition only beyond the ~1000 ppm unaided lock range and only
      when confirmed. Timing jumps (samples lost/inserted by network streams) are
      detected from the cyclic-prefix correlation and a time-pilot monitor and
      resynchronised in ~1 s without a full restart (`examples/dropout`).
      Sensitivity (`examples/bercurve.rs`, BER 1e-4 after decoding, 64-QAM R = 0.6,
      2 MLC iterations, real synchronisation and channel estimation, modes A for ch1-2
      and B for ch3-5) against ES 201 980 annex A (ideal estimation): ch1 14.8 dB
      (14.9), ch2 15.8 (16.5), ch3 23.8 (23.2), ch4 23.8 (22.3), ch5 22.0 (20.4) — at
      most 1.6 dB from the ideal-receiver figures (pooled over 3-4 seeds × 300-600 s per
      point, 0.1 dB grid for ch1-2; earlier 60 s runs had ch4 1.5 dB too optimistic).
      Soft metric (2026-09-30, same runs, paired seeds; `MetricKind`): Dream's
      |r/h − s|·|h| against the Euclidean |r/h − s|²·|h|²: Dream's is 0.1-0.35 dB
      better at BER 1e-4 on every channel; the Euclidean curve is steeper with the
      lower floor on ch1-2 (better at MSC frame error rate 1 % there, ~0.1 dB worse on
      ch3 and ch5). A Huber shape (squared up to δ, linear beyond; δ = c × the level's
      subset half-distance, so only abnormally large distances — from wrong decisions
      of other MLC levels — are clipped) recovers Dream's waterfall on AWGN but, with
      |h|² weighting, loses 0.2-0.46 dB on the fading channels: the weighting is what
      Dream gets right (|h|² over-trusts strong carriers there). With Dream's |h|
      weighting (`HuberAmplitude`), c = 0.25 is never worse than Dream at BER 1e-4
      (−0.01 to −0.07 dB) and 0.17 / 0.11 dB better at FER 1 % on ch1 / ch2 (neutral
      within ±0.05 dB on ch3-5; 16-QAM checked on ch1/ch3): now the MSC default
      (`rx::MSC_METRIC`); FAC and SDC keep Dream's metric. The recordings decode
      identically (their errors are not SNR-limited).
- [x] **M3 Codecs & text** — xHE-AAC, Opus, text messages, concealment, drift-compensated
      live playback. *Done* (xHE-AAC on FMGold, Opus in all three signalling variants).
- [x] **M4 Data services** — Journaline, MOT Slideshow, BWS, EPG, TPEG/raw; clock & AFS.
      *Done:* decoders verified on recordings (slideshow, Journaline, BWS) and with our
      transmitter (EPG, TPEG, raw applications, alternative frequencies — there are no
      recordings of those here). TPEG and uninterpreted applications are captured to
      `<data dir>/raw/` (data fields of the CRC-checked data groups; stream-mode bytes);
      the station sends them from a file (`type = "tpeg"` / `"raw"` with `app_id`).
      Clock and AFS (SDC types 3, 4, 7, 11; the station's `[afs]` section) shown in the
      CLI and the GUI (`Snapshot::afs`, `time`).
- [x] **M5 Live input & logging** — sound-card input (VAC), CSV/JSON logs & metrics.
      *Done:* sound-card input (CLI `--device`, GUI); `decdrm rx --log FILE.csv|.jsonl`
      writes metrics rows (and events in JSON Lines).
- [x] **M6 GUI** — egui: spectrum/waterfall, constellations, SNR/MER, sync status,
      service list, text, slideshow, Journaline browser, EPG, clock/AFS.
      *Done:* receiver tab (spectrum, waterfall, constellations, decoded-audio
      spectrum, channel, impulse response, SNR per carrier, history of the last five
      minutes of signal with error rates per 10 s, LEDs, Dream-style service bars (codec,
      SBR/PS, rates, bit rate, EEP/UEP, text, data applications, CA), text, slideshow,
      Journaline, broadcast website (opens the HTML start page in the system browser on
      a click), EPG, broadcast clock with local time, alternative frequencies, playback
      volume, data
      directory) and transmitter tab (TOML editor with error locations, validation,
      transmit to file/sound card, status, TX spectrum).
- [x] **M7 Transmitter** — full TX chain, FDK AAC/HE-AAC encoding, all data services,
      file/sound-card output, channel simulator, loopback BER tests; GUI TX tab + CLI.
      *Status:* `decdrm-station` + `decdrm tx station.toml` + GUI tab done
      (AAC/HE-AAC/v2 up to AAC's bit-rate limit, Opus, slideshow, website, Journaline,
      EPG, text, time; all MSC modes incl. HM/UEP), verified by loopback through our
      receiver, also live through a virtual audio cable. A sound-card input with a
      sound-card output is drift-compensated: a PI loop trims the input resampler to
      hold the capture backlog at 0.3 s (primed by the output queue at start-up); live
      85 s run: trim within ±160 ppm, 0 underruns.
- [x] **M8 TX codecs** — xHE-AAC (libxaac encoder), Opus.
      *Done:* station codecs `xhe-aac` (24/32/48 kHz by stream rate, mono/stereo,
      text) and `opus`; `XheAacFramer` writes the header reservoir level itself and
      refuses to pad mid-frame. Loopbacks mode B/A/D decode with 0 concealed frames.
      Not supported: MPS212 (4:1 stereo), 38.4 kHz stereo.
- [x] **M9 EnCodec** — experimental neural-codec extension (TX + RX).
      *Done:* `decdrm-encodec` (feature `encodec`, candle 0.9, own streaming SEANet
      identical to candle-transformers' EnCodec), signalled as SDC type 9 audio coding
      10 with an `ENC1` config (Dream-safe), 1.5–24 kbit/s tiers, per-region CRC-8,
      optional repetition of the base layers, latent interpolation for lost frames.
      At 15 dB (mode B, 64-QAM) 2.1 % of frames concealed against 26.5 % for HE-AAC.
- [x] **M10 Polish** — performance, Linux verification, docs.
      *Done:* performance measured, no hot spot worth optimising: the receiver decodes
      10 kHz signals at 100–150× real time and 20 kHz ones at ~45× on one core;
      EnCodec encodes/decodes at ~15× real time; the GUI needs 2–6 % of a core while
      decoding live (~12 frames/s, < 1 ms per frame; a frame per input event while the
      mouse moves). Linux: the manual GitHub Actions workflow builds, tests and lints
      the workspace, then smoke-tests device listing without a sound card, a CLI
      transmit → receive round trip and the GUI under Xvfb/Mesa (screenshot artifact).
      Docs: `docs/USER_GUIDE.md`; rustdoc builds without warnings. The station's
      `[simulate]` section (and `decdrm tx --channel-model/--snr`) exposes the channel
      simulator as a test-signal generator; its clock error is applied to the output
      samples, so it scales the IF like a real sound-card clock.
- [x] **KCBS EVS** (2026-10-01) — Korean Central Broadcasting on 6140 kHz signals its
      only service as data (user application 0x000) but sends 3GPP EVS audio.
      *Done:* framing reverse-engineered from recordings (`decdrm_evs::kcbs`: one
      716-byte data group per 400 ms with 20 EVS frames of 264 bits = 13.2 kbit/s,
      super-wideband ACELP, the first byte of every fourth frame moved to the front; a
      52-byte side channel with a 1.2 s counter). Confirmed with the 3GPP decoder
      against a random-payload control (voiced pitch strength 0.56 vs 0.27, spectral
      flux 7.8 vs 12 dB). `decdrm-evs` builds the TS 26.443 float decoder from the zip
      in `reference/evs/` (feature `decoder`; engine/CLI/GUI feature `evs`) behind a
      frame-by-frame shim that matches the reference program to 1 LSB. The engine
      locks a data channel as EVS after two matching data groups (all 20 frames
      signalling one bandwidth; random data ~10⁻⁸), plays it like an audio service
      (lost data groups concealed) and shows it as EVS in the service bars.
      *Then:* the decode sounded glitchy because the station's encoder departs from
      EVS in INACTIVE, TRANSITION and LR-MDCT frames (the decoder's bit-error checks
      fire on 34 %/42 % of the INACTIVE/MDCT frames, 0 % for the 3GPP encoder; TRANSITION
      frames clip; EVS 12.0–12.2 decoders do no better). `KcbsDecoder` conceals those
      types, a burst guard (one frame of delay; on a clipping or +15 dB frame a fresh
      decoder replays 0.5 s and conceals it and its predecessor) removes the rest (0
      clipped frames on the recordings, from ~47 per 32 s), and concealed pauses get
      comfort noise: spectrum and level measured from the pause frames a second decoder
      decodes normally (outliers rejected; −61 dBFS, hum-weighted), 129-tap FIR-shaped
      white noise 3 dB below it, 10 ms fades.
      *Removed* (2026-10-01, at the user's request, so the repository can be public):
      the decoder build (feature, shim, `KcbsDecoder`, burst guard, comfort noise) and
      its playback in the engine. Recognition stays: the framing, the signalling
      table, the service shown as EVS with "no decoder" and "likely encrypted", and the
      findings above, recorded in `decdrm_evs::kcbs`.
- [x] **Station schedule** (2026-10-01) — which DRM stations are on the air now, like
      Dream's Stations dialog (`StationsDlg.cpp`, `Schedule.cpp`), to know what to
      tune a web SDR to.
      *Done:* `decdrm-schedule` reads Dream's `DRMSchedule.ini` as `ReadINIFile` does
      (Sunday-first day flags, `0000000` = irregular = every day; bad records skipped
      instead of ending the file) and EiBi's seasonal CSV as `ReadCSVFile` does, with
      Dream's code tables (`TableStations.cpp`, generated into `eibi_tables.rs`) for
      languages, targets, countries and sites; columns found by header name, lenient
      days (`Mo-Fr`, `Sa,Su`, `SaSu`, `1245`, `Fr-Mo`, keywords) and validity dates
      (`ddmm`, as EiBi's `README.TXT` says and the A26 file confirms; a `[mmyy]` after
      the stop date is the month last heard, shown as a note); DRM = EiBi's mark
      `DIGITAL` after the station name (`BBC DIGITAL`; not in the README, and the A26
      file has no "DRM" at all — the first version, looking for that word, found 0 of
      its 118 DRM entries), or the word "DRM" in station, remarks or language. On
      air: the start day decides (Dream misses Friday's 2300-0100 on Saturday at
      00:30), `start == stop` is all day, a single annual validity date is taken within
      the broadcast season and two make a window that recurs every year (compared by
      month and day, so a permanent entry's `05-15`…`12-20` holds in either season's
      file), Dream's ending-soon (10 min) and
      starting-soon (15 min) states. UTC calendar from `SystemTime` (civil-from-days),
      seasons A/B from the last Sundays of March/October (`sked-a26.csv`). Sources
      (EiBi's current season file, Dream's DRMDX URL on baseportal.com) configurable
      in `sources.toml` in the per-user `schedule` folder; downloads only on request,
      via curl/wget into a `.part` file that replaces the old copy only if it parses.
      CLI `decdrm schedule` (`--update --all --at --freq --source --all-broadcasts
      --filter --sources --dir`); GUI *Schedule* tab (background load/update, rows
      cached per minute and painted with `show_rows`, the received frequency from the
      *Frequency* box or the recording's file name — KiwiSDR, HDSDR, SDR# naming —
      highlighted and named in the log at Start; a click copies the frequency).
      EiBi's README codes (`entry::Activity`, `MonthDays`): persistence code 4 = only
      in the winter (B) seasons, 5 = only in the summer (A) seasons, 8 = inactive (never
      on the air, like `alt`, an alternative frequency not usually in use), 90 + a code
      = utility station; a single date on codes 1–5 (copied into every season's file)
      is a note, not a bound; Days forms `1.Sa`/`2.Su` (the n-th weekday of the month),
      `1WeFr` (the first Wednesday and the Friday after it), `Last7`, `MF-15` (Monday to
      Friday up to the 15th), `15Sep` (that day only), `altFr` (every Friday, noted as
      alternate weeks). In the A26 file this takes the 359 winter-only entries (among
      them CNR1 DIGITAL on 15180 kHz and Scandinavian Weekend Radio's winter schedule)
      and the 80 inactive ones (SE-TA2 DIGITAL) off the air.
- [x] **Fading and delay–Doppler maps** (2026-10-01) — two displays Dream lacks.
      *Done:* `rx::scatter`: the last 6 s of channel estimates (one row per symbol,
      brought to the newest timing with each row's cumulative shift) → Hann-windowed
      inverse FFT over the carriers (delay, 2× zero padding) → Hann-windowed FFT over
      time per delay (Doppler, ≥ 4× zero padding), once a second (with every new symbol
      since the 60 Hz plots); dB below the strongest point, floor −40 dB, delays
      −Tg/4 … 1.25 Tg, Doppler ±5/8/10 Hz by mode. GUI tabs *Fading* (gain per carrier
      per snapshot, a minute, colours from the median gain; a row per symbol since) and
      *Delay–Doppler*. Checked against channel model 3 (four paths, spreads
      0.1–2 Hz, in place) and on KCBS (separate ionospheric paths). The waterfall also
      fits the DRM signal now, at the spectrum's full 2048-bin resolution.
- [x] **RF monitor** (2026-10-02) — the user asked for a button that passes the I/Q
      through to the output to hear it. *Done:* `Command::SetMonitor` /
      `EngineConfig::monitor`: `AudioOut` plays the first branch's input frames (48 kHz,
      as they come in: I left and Q right, mono on both sides) instead of the decoded
      audio, which is still recorded and analysed; file playback stays paced by the
      sound card; snapshot `audio.monitor`. GUI: *RF monitor* toggle beside *Record…*
      (amber while on; *Output: RF monitor*; disabled without audio and for MDI/RSCI),
      `decdrm-gui --start --monitor`. Tests: the recording keeps the decoded audio with
      the monitor on; through VB-Audio cable A (ignored test, inaudible) an I/Q file of
      1 kHz on I and 3 kHz on Q came back 1 kHz left and 3 kHz right at the input's
      level, paced in real time.
- [x] **App icon** (2026-10-25) — DecDRM had none. Original artwork in `assets/icon/`:
      a DRM OFDM spectrum (teal carriers, three boosted amber gain pilots), the middle
      pilot sending radio waves, on a rounded dark-blue square. `decdrm.svg` (32 px and
      up) and `decdrm-small.svg` (16/24 px, drawn on a 16-pixel grid);
      `scripts/make-icon.py` renders `png/decdrm-{16..256}.png` with resvg's CLI
      (`cargo install resvg`) and packs them into `decdrm.ico` (PNG entries). The GUI
      sets the 256 px PNG as window/taskbar icon (`ViewportBuilder::with_icon`, compiled
      in); `build.rs` in both apps embeds the .ico and version information in the
      Windows exes (`winresource`, only when the target is Windows, so Linux builds
      skip it). Checked: Explorer's icon of both exes, `WM_GETICON` of the GUI window.
- [x] **DAC joint stereo** (2026-10-25) — the user asked for joint stereo in the DAC
      codec (someone else showed "DAC joint stereo, 12 kbit/s mid + 1.5 kbit/s side,
      24 kHz" without sharing code; written here from scratch).
      *Measured first* (stereo music: an Opus test transmission decoded to WAV, 36 s,
      side 8–10 dB below mid; whole-signal DAC, 16-codebook mid): left/right
      log-spectral distance 5.2 dB with no side (mono), 2.8 with a 2-codebook side
      (1.5 kbit/s), 2.3 with 4, 1.8 with 8. Raising the side's level by 6 or 12 dB
      before coding did not help (6 dB: the same, 12 dB: worse), so no side gain.
      *Done:* `DacConfig::side` (`SideCoding`), signalled as a second parameter byte
      after the first (`\0DAC1 pp ss`, same layout; format 1 receivers ignore it and
      play the mid signal). Super frame = the mid block exactly as a mono super frame,
      then the side block with its own CRC regions and repetition
      (`framing::pack_super_frame` / `unpack_super_frame`). Planner
      `choose_stereo_config`: side = an eighth of the mid's rate, at least 1.5
      kbit/s (`default_side`), the largest mid that fits with it, spare bytes to the
      mid's framing first. `DacDrmEncoder` takes interleaved stereo and codes M/S with
      two encoder states under `rayon::join`; `DacDecoder` keeps a `Channel` (network
      state + concealment memory) per signal, decodes them side by side, outputs
      L = M + S, R = M − S. A lost mid frame drops its side frame too; a side lost
      alone is concealed (bridged, faded), which fades to mono. Station: `stereo =
      true` with DAC, `side_bandwidth_kbps`; GUI form *Joint stereo* + *Side bit
      rate*; engine shows "DAC joint stereo, 12 kbit/s mid + 1.5 kbit/s side, 24 kHz"
      and a Stereo tag. The SDC's audio mode stays 00 (rfa for coding 10).
      *CPU:* this machine is a Ryzen 7 4800H laptop (the RTFs above were a 9800X3D):
      mono 12 kbit/s decode RTF ≈ 0.6, joint stereo ≈ 0.95–1.0, encode 0.35 / 0.54.
      Stacking both signals' rows into one matrix product was no faster than
      `rayon::join` (micro-benchmark per layer shape), so no batching. The matrix
      products are about two thirds of decode time; the decoder's first transposed
      convolution (30 rows × 1536 × 12288 per super frame) is now computed as
      (Wᵀ·xᵀ)ᵀ: 23 → 6.5 ms, identical output (decode RTF 0.63 → 0.58 here).
      Tests: config/framing/plan units (round trips, a mono reader of a stereo super
      frame, side errors leave the mid alone, planner monotonic), model test
      `joint_stereo_round_trip` (tones on their sides, 18 dB separation at 12 + 3;
      a mono decoder gets exactly (L + R) / 2 from the same super frames; garbage side
      blocks → 60 side frames concealed, mono at full level), station planning and
      `dac_joint_stereo_mode_b_loopback` (mode B 64-QAM, 12 + 3 kbit/s: tones at
      ~23 dB separation, no frame concealed). With a 1.5 kbit/s side the same two
      steady tones separated by only ~6 dB: low side rates suit music better than
      test tones.
- [x] **RSS feeds as Journaline pages** (2026-10-25) — someone's DecDRM fork carries
      RSS feeds in Journaline and does not share its code; the user asked for the same
      feature, written here from a 60 s recording of its transmission only.
      *The recording* (`drm_station.wav`, mono 48 kHz real IF, DC carrier 10 kHz,
      decoded with `decdrm rx` and the new diagnostic `--example jmldump` of
      decdrm-engine, which prints every Journaline data group with its NML codes
      shown): mode B, SO3, 16-QAM, long interleaving, a DAC audio service "WWWW" and a
      data service "Data 2", id 0xD0D002 (the GUI's defaults for a new data service),
      Journaline in packet mode, packet id 0, 45-byte packets (DecDRM's default), 4.80
      kbit/s (4800 bit/s requested is exactly 5 packets per frame). Their GUI shows
      the root "News feeds" with three feeds and 34 pages, as ours does.
      Page tree, object ids given out in order: 0 = menu "News feeds", one entry per
      feed, the entry text the channel's `<title>` ("BBC News", "NBC News Top
      Stories", "TIME"); then for each feed its menu (title = channel title, one entry
      per item, the item's title in full) followed by its items' pages: BBC menu 1,
      items 2–11; NBC menu 12, items 13–22; TIME menu 23, items 24–33. Ten items per
      feed (all three feeds have more). Each item is a plain-text object (type 2):
      title = item title, body = description, two NML line breaks (0x10 0x10), the
      item's link; an item without a link has the description alone. No dates, no
      title-only pages, no list objects, no links other than menu links. Text is
      UTF-8 as in the feeds (typographic quotes, dashes, "…" kept), entities and
      CDATA resolved, no HTML left, white space trimmed; nothing was shortened (titles
      up to 103 bytes, descriptions up to 303, pages up to 527 bytes, menus up to
      882). Transport exactly as DecDRM's own `JournalineEncoder`: type-0 MSC data
      groups with CRC, no extension, segment or user access field, one object each,
      continuity counter per group, objects in id order (34 objects, a cycle of 23.3
      s), uncompressed, static flag 0, no extended header, revision 0 throughout (no
      update within the 60 s, so the refresh period is not observable). Nothing on
      the air departs from TS 102 979 or from what DecDRM already sends; the link is
      plain text in the body. One oddity: on the NBC feed each page's title belongs
      to the item before (its description and link agree with each other, the title
      not), and titles repeat — apparently their parser's doing on that feed's markup
      (probably namespaced `media:` elements); not reproduced: here title,
      description and link always come from the same item, and only plain or Atom
      elements directly inside the item count (`media:title` etc. are ignored).
      *Done:* `decdrm_station::feeds` — parser (quick-xml, the GUI's version) for
      RSS 2.0, RSS 1.0/RDF and Atom: item title, description (Atom summary; else
      `content:encoded` / Atom content), link (Atom `link` without `rel` or
      `alternate`; else a permalink `guid`); documents declared ISO-8859-1 /
      windows-1252, or not UTF-8, converted; damage after the first items keeps
      those. Text: HTML tags removed (`<br>`, block ends → line breaks in
      descriptions), XML and common HTML entities decoded, control characters
      dropped, white space collapsed. `feed_pages` builds the recording's tree; a
      title or entry is clipped at 200 bytes, a description shortened with "…" to
      keep the page within 4092 bytes, a menu that would not fit loses its last
      items. Ids are positional, as in the recording: an item keeps its page while it
      keeps its place, and a new item at the top changes the pages after it (each
      with the next revision, sent first by the carousel). `FeedWatch`: the first
      fetch in `Station::new` (none of the feeds fetched = the station's error, some
      failing = logged, tried again with the others), then a thread fetches every
      `feed_refresh` seconds (webstream's HTTP/1.1 + rustls client, at most 4 MiB a
      feed, the stop flag on drop) and the frame loop loads each finished fetch into
      the carousel with `replace_all` (the *Live Journaline updates* path); a feed
      that fails keeps its pages. `JournalineWatch` takes a page file or feeds;
      `reload_journaline()` (GUI *Update*) asks for a fetch now. Config on the
      Journaline application: `feeds = [...]` (instead of `path`), `feed_refresh`
      (default 600 s, at least 60), `feed_items` (default 10, 1–50), `feed_title`
      (default "News feeds"); at most 20 feeds. Status `JournalineStatus::feeds`
      (`FeedStatus`: URL, title, items, error), the feeds' errors as the
      application's error. GUI: the Journaline row's *Page file / RSS feeds* switch,
      the URLs one per line, *Every … min*, *… items*, the root title; the
      Transmitter tab shows "N pages from K feeds" with each feed in the tooltip. CLI
      status lines say "from K feeds". The receiver needed nothing new. Tests (all
      on 127.0.0.1): parser units (RSS with `media:title` and an `<image>` title,
      Atom with several links and XHTML content, RDF, Latin-1, damage, HTML), the
      page tree and its bytes against the recording's, size limits, `FeedWatch`
      (schedule, a failing feed keeps its items, fetch on request, nothing fetched
      fails), `JournalineWatch` with feeds (log, status, revisions), the GUI form,
      and `journaline_feeds_loopback` (an RSS and an Atom feed, mode B 16-QAM 4.8
      kbit/s like the recording, a new item while transmitting: the receiver gets
      the tree, then the changed pages with revision 1 and the new page). Checked
      by hand: `decdrm tx` with feeds from a local server, `jmldump` of its output
      shows the recording's layout.
- [x] **Corrupt xHE-AAC frames concealed; Smooth SBR** (2026-10-07) — from the overnight
      CNR survey (2026-10-06/07: 6030 and 13835 kHz on Korean, Japanese and Taiwanese
      Kiwis, decoded by FDK-AAC and an instrumented libxaac). (1) The session handed
      xHE-AAC frames whose CRC-16 failed to FDK-AAC, which does not check it, and counted
      them as good; on fading channels that gave bursts up to 37 dB above the programme
      (scored against clean captures of the same broadcast, also with white noise added).
      *Done:* such frames are concealed (§5.3.3) and counted as concealed; the price is more
      short mutes at marginal SNR, where FDK fades back in over 5 frames (its concealment
      method cannot change that: energy interpolation is refused for xHE-AAC, spectral
      muting is no better than the default). Test `xhe_frame_failing_its_crc_is_concealed`
      damages one frame's CRC with its access unit intact. (2) CNR-1 on 13790/13835 kHz
      sounds glitchy because its encoder makes the SBR band jump by more than 18 dB between
      consecutive 64 ms frames about once a second (and drops speech to digital silence for
      144–192 ms now and then); 6030 keeps its SBR band quiet and steady. The user asked for
      an option. *Done:* `decdrm_io::HighBandSmoother` — linear-phase FIR split at the SBR
      crossover (`AudioParams::sbr_crossover_hz`: the core's Nyquist frequency), high-band
      level in 16 ms blocks, a lower envelope rising and falling by at most 3 dB per block
      with 4 blocks of look-ahead, gains from each block's centre to the smaller neighbour
      at its edges, attenuation only; ≈100 ms of delay, the output as long as the input.
      On a 13835 recording: audible high-band jumps 53 → 3.5 a minute, the band ≈11 dB
      quieter, the audio below 5 kHz unchanged (−66 dB). 8 or 4 ms blocks did worse (noisier
      levels, larger steps where frames straddle blocks); centre-to-centre interpolation
      let a band switched on inside a block through (27 dB steps). `Session::set_smooth_sbr`,
      `Command::SetSmoothSbr`, `EngineConfig::smooth_sbr`, `decdrm rx --smooth-sbr`; the GUI's
      *Smooth SBR* button (Audio panel) is remembered per station by service ID
      (`smooth_sbr_services` in the settings). Tests: the smoother (steady audio only
      delayed, a 40 dB gated band smoothed and never raised, stereo and any chunking, flush),
      `sbr_crossovers`, `sbr_smoothing_delays_and_keeps_frame_lengths`.
- [x] **xHE-AAC decoder cross-check** (2026-10-06) — the user heard glitchy xHE-AAC from
      CNR-1 (Qiqihar: 13790 kHz until 10:00 UTC, then 13835 kHz; via bv3un.ddns.net) at a
      good SNR without errors. 60 s on 13835 kHz: mono, 32 kHz, 8:3 SBR with the harmonic
      transposer (eSBR), 11.6 kbit/s; 888 frames, every CRC-16 good, none missing. The
      second opinion came from libxaac's decoder (`third_party/libxaac`, its test program
      built with CMake), which knows USAC but not DRM. *Done:*
      `audio_specific_config_from_drm` (decdrm-codecs) turns an xHE-AAC Static Config back
      into a standard AudioSpecificConfig: `tw_mdct` = 0 inserted, the channel element
      first and the extension elements after it, MPS212 in MPEG syntax
      (`bsTempShapeConfigDrm` 1 → `bsTempShapeConfig` 3, `bsDecorrConfig` 0, as FDK reads
      it), the config extension copied. `examples/xhedump` (decdrm-engine) writes a
      recording's frames, FDK's output and libxaac's input (`.usac` + `.meta`). libxaac
      decodes CNR-1 like FDK (median difference per frame −48 dB after a 160-sample delay;
      FMGold, stereo 4:1 SBR with MPS212: −65 dB), so the glitches are in the broadcast.
      ffmpeg 9.0.1 has no eSBR. Tests: CNR-1's config as a fixed vector; FDK decodes
      DecDRM's own xHE-AAC frames as plain MPEG USAC with the converted config bit-exactly
      as in DRM mode (mono and stereo; no SBR, 8:3, 2:1, 4:1); the MPS212 and extension
      syntax by hand. FDK's MPEG path decodes no 19.2 kHz stream, not even libxaac's own.
- [x] **Audio shares in the Transmitter form** (2026-10-04) — the user asked to change
      how audio services split the capacity instead of 50/50. The station already
      weighted them by `share`; the form did not show it. *Done:* with two or more
      audio services (not counting one in the hierarchical layer), each gets an *Audio
      share* slider (1–99 %) and its stream rate from the last check. Moving one writes
      every audio service's `share` as percentages, the others keeping their proportions
      among themselves, rounded to 0.1 (`rebalance`). An audio service added after
      shares were set gets the others' mean share. Test `audio_shares`: 70/30, then a
      third service at 20 % gives 56/24/20, and the station's plan splits the stream
      rates in those ratios.
- [x] **Testing and releases on GitHub Actions** (2026-10-03) — the user asked to move
      the testing to GitHub Actions. The repository is public, so the runners are free;
      the old Linux workflow ran only by hand, to save a private repository's minutes.
      *Done:* `ci.yml`, on every push to main and every pull request (and by hand):
      Linux and Windows build the workspace and run its tests, the long loopback and
      diversity sweeps (ignored locally), clippy (correctness lints as errors) and
      `scripts/smoke-test.sh`: `decdrm devices` without a sound card, the example station
      transmitted and received without a concealed frame, and the GUI decoding it
      headless (Xvfb on Linux, Mesa's software OpenGL on Windows), with the logs and a
      screenshot as artifacts. A Linux job runs the DAC tests with the real weights
      (`.github/actions/dac-weights`: from the cache, else downloaded from Hugging Face
      at the fixed revision; SHA-256 checked), then the smoke test of a DAC station
      decoded from weights built in. `release.yml`, on a pushed tag v*: the portable
      Windows build of `scripts/build-portable.ps1`, then checks: versions against the
      tag, no `vcruntime140` or `api-ms-win-crt` import (static C runtime), and the
      smoke test with the built-in weights alone. Then a draft release with both exes
      and their SHA-256; a published release is never replaced. Not on CI: the
      sound-card tests, the measurement printouts and anything that needs the user's
      recordings.
- [x] **Non-standard FAC identity** (2026-10-03) — the user found a station on 1557 kHz
      (two Taiwanese KiwiSDRs in diversity; mode B, 9 kHz, 16-QAM MSC, 4-QAM SDC, one data
      service "AMDrm") whose SDC constellation kept switching between 4-QAM and 16-QAM. In
      the user's video the SDC plot refreshed every 400 ms instead of every 1.2 s, showing
      the SDC in one frame and MSC cells in the other two: every frame was taken for the
      first of its super frame, so the station's FAC identity must have been 10 ("last
      frame") in every frame. The receiver, like Dream (`FAC.cpp`, `OFDMCellMapping.cpp`),
      followed the identity: it decoded an SDC block in every frame (one in three passed:
      yellow SDC light, the label arrived) and restarted the super frame at every FAC, so no
      multiplex frame was ever completed (grey MSC light). *Done:*
      `SymbolChain::check_identity`: after three good FACs in a row whose identity
      contradicts the frame count, the identity is no longer trusted. The frames are then
      counted, and every frame's first symbols are tried as an SDC block until one passes its
      CRC; that frame starts the super frame. After three failures in a row there, the other
      frames are tried again while the MSC carries on. Six identities in a row that agree
      with the count restore the trust. No multiplex frames while the frame index is in
      doubt. `ReceiverEvent::FrameIdentity`, logged. Tests: loopback `fixed_fac_identity`
      (identities 0–3 via `Transmitter::set_fixed_fac_identity`). Identity 2 gave 9 good and
      19 bad SDC blocks and no multiplex frame in 12 s; identities 0, 1 and 3 gave no SDC at
      all. Now each gives 23/23 multiplex frames bit-exact. Diversity
      `combining_with_a_fixed_fac_identity`: 89 frames combined against 92 with a standard
      identity. The 25 recordings and the loopback sweeps decode exactly as before, and none
      triggers it. `decdrm-kiwi --example capture` records Kiwis' I/Q to WAV files. On 60 s
      of the station from bv3un.ddns.net (2026-10-03 ~12:00 UTC, SNR 6–12 dB) every FAC
      printed said frame 2. Before: SDC 33 good and 85 bad, 20 multiplex frames. Now: the
      identity is distrusted after 1.2 s; SDC 33 good and 16 bad, 124 multiplex frames. Its
      one data service has no application information in the SDC (no type 5 entity;
      the FAC's application identifier is 1), so there is nothing to decode: a test
      transmission.
      *False FACs:* the same log showed one FAC with 16-QAM SDC and three services for a
      single frame: corrupted bits that passed the 8-bit CRC, as one corrupted block in 256
      does. `Receiver::fac_confirmed`: a FAC with another channel configuration (layout,
      modes, interleaving, number of services) is passed on only once the next good FAC
      brings it too, so a real reconfiguration takes effect a frame later (test
      `single_odd_fac_not_passed_on`). Diversity: the MSC plot shows the combined cells only
      while multiplex frames are decoded (it had stayed on a stale frame), else the branch's.
- [x] **DAC replaces EnCodec** (2026-10-02) — the user asked whether SemantiCodec
      would beat EnCodec ("if so replace it"). It would not here: at most 1.40 kbit/s
      with 16 kHz output, ViSQOL 3.48 against EnCodec's 3.58 at 3 kbit/s and 4.00 at 6
      in its own paper, whole 10.24 s windows (about 20 s delay), a diffusion decoder
      (75 M-parameter UNet, 50 DDIM steps × CFG, ~200 GMAC per second of audio), 2.5 GB
      of weights. DAC (descript/dac_24khz, MIT code and weights) was proposed instead
      and the user said to go ahead.
      *Comparison* (`examples/codec_compare` at commit 71de0ac; five inputs: decoded
      Opus music and HE-AAC speech from the DRM samples, xHE-AAC music, Windows TTS
      speech; EnCodec streaming as DecDRM ran it, DAC whole-signal), mean
      multi-resolution mel distance EnCodec → DAC: 1.5 kbit/s 0.291 → 0.245, 3: 0.260 →
      0.209, 6: 0.233 → 0.170, 12: 0.211 → 0.129, 24: 0.198 → 0.093; log-spectral
      distance, mel-cepstral distortion and SI-SDR agree at every rate (the two music
      clips tie up to 6 kbit/s). DAC at 3 kbit/s ≈ EnCodec at 12; DAC at 6 beats EnCodec
      at 24. Lost 40 ms groups concealed by latent interpolation for both: at 12 kbit/s
      with 20 % lost DAC 0.203, EnCodec without losses 0.211 (DAC degrades more, from a
      better start). Listening excerpts were written to `out/codec_compare` (not in
      git). Speed (Ryzen 7 9800X3D): DAC decode RTF 0.24 on 16 threads, 0.42 on 4, 0.83
      on 1; encode 0.15 / 0.41; EnCodec 0.07 on one core.
      *Done:* crate `decdrm-encodec` renamed `decdrm-dac` (feature `dac`), keeping the
      DRM side (tiers, framing, CRCs, repetition, concealment, planning) and replacing
      the model: `model.rs` streams DAC exactly — time-major activations, each layer
      keeping the rows it still needs and emitting what its look-ahead allows (decoder
      lag 3141 samples, encoder look-ahead 2493, so a stream starts with 2560 samples
      of silence and then gives one frame per 320 samples), `flush` for the end
      padding; tested against `model/reference.rs` (candle's own convolutions over the
      whole signal): identical codes, decoded audio within 1e-4 in odd chunk sizes.
      candle's convolutions were the bottleneck (single-threaded im2col gather and
      transposes, a direct loop for padded transposed convolutions), so only the matrix
      products go through candle and the rest are parallel passes (rayon): decode RTF
      0.78 → 0.24 on 16 threads. The decoder primes its output with the look-ahead as
      silence and delays the concealment gains with the samples. New CRC policy
      `Adaptive` (default): decode every code as received unless at least 90 % of the
      super frame's regions failed (garbage), then trust enhancement + interpolation.
      With DAC, decoding a burst's few wrong codes beat concealing the frames (6 kbit/s,
      no repetition, bursts 10⁻³/bit: 1.28 dB / −29.4 against 1.72 / −27.5 for the old
      default), garbage super frames are still better concealed (30 %: 5.61 / −28.1
      against 6.86 / −27.2 ignoring the CRCs); adaptive behaves as the first under
      bursts and as the second for garbage. With the base layer sent twice (6 and 3
      kbit/s) adaptive and the old default stay within 0.03 dB LSD up to 3·10⁻³/bit,
      adaptive with the better NRR, and conceal garbage alike; only at 10⁻²/bit does
      the old default give the lower LSD (7.18 against 7.64, NRR −6.6 against −9.3).
      Signalled as audio
      coding 10 with `\0DAC1` (`AudioCodec::Dac`); the old `\0ENC1` services are
      recognised as `AudioCodec::Encodec` and shown "no longer supported"; station
      files with `codec = "encodec"` get an error naming DAC. Weights: 299 MB,
      `decdrm models download dac`; decoder half 209 MB loaded alone by receivers.
      Tests: crate units, round trip through the framing (delay 5693 samples = lead-in +
      look-ahead − 8, LSD 1.0–3.7 dB), streaming = whole signal (3600 of 3600 codes,
      decoder difference 0), tiers (LSD 8.7 → 1.4 dB from 1.5 to 24 kbit/s), bursts
      (CRCs: 43 of 43 corrupted regions caught; trusting enhancement layers LSD 1.04 vs
      1.59 dropping them), concealment fades; station loopbacks mode B 12 kbit/s and mode
      D 6 kbit/s with no frame concealed; under noise (mode B, 64-QAM, protection 1,
      12 kbit/s with codebooks 0–7 twice, against HE-AAC on the whole 20 kbit/s):
      15.5 dB 0 % concealed (HE-AAC 0.4 %), 15.0 dB 1.4 % (37 %), 14.5 dB 44 % (100 %) —
      as with EnCodec, since the framing decides it.
- [x] **MDI / RSCI / DCP** (2026-10-02) — the user's choice of next feature: both
      input roles (the receiver ← MDI/RSCI, the transmitter ← MDI as a modulator),
      full RSCI status (Dream parity), GUI and RCI control (no CLI flags), the ETSI
      specs to be cross-checked once downloaded (TS 102 820, 821, 349).
      *Done:* new crate `decdrm-mdi`: AF packets, PFT (fragmentation, addressing,
      RS(255,207) shortened, GF poly 11D, roots α⁰…α⁴⁷; byte j of fragment i is byte
      j·F + i of the RS block; Karn's errors-and-erasures decoder, lost fragments as
      erasures), TAG items, MDI frames (`*ptr dlfc fac_ sdc_ sdci robm str0-3 info`),
      RSCI status (every item Dream reads or writes, `rpil` included), RCI commands
      (`cact cfre cdmo crec cpro cser`), recordings (TS 102 821 annex B file framing,
      raw AF/PFT, pcap and pcapng with IPv4 reassembly), UDP in Dream's origin syntax
      (multicast join on an interface, source filter in software; std only), dlfc
      repeat/loss logic as Dream's. Engine: `InputSpec::Mdi`, `Session::new_mdi`/
      `push_mdi` (FAC/SDC into the ensemble, the streams to the decoders via the
      demultiplexer-free `on_logical`), RSCI items as the receiver status (MER, WMER,
      FAC WMER, Doppler, delay, sync) and plots (`rpsd` at −7.875 kHz + 0.1875 kHz per
      value relative to the DC carrier, a waterfall row per frame; `rpir` as the PDS),
      tune and service selection to the RSCI receiver by RCI, `EngineConfig::
      rci_listen` (RCI commands → engine commands), snapshot `input.mdi`, `remote`.
      Station: the modulator (`[mdi]`: input, buffer_frames; alignment by the FAC frame
      identity, fillers for lost/late/damaged frames, dlfc for late ones, transmitter
      and output stage rebuilt on a channel change at a super frame start, a reserve
      and super-frame trimming with a sound card, services from the MDI's FAC/SDC in
      the status), `capture_mdi` (a frame as MDI) and the `mdi_source` example. GUI:
      *MDI/RSCI* source (address, recording picker, RCI address, frequency → RCI),
      MDI recordings via *Open…*, status strip (link, profile, signal, frequency,
      receiver name, details), ⚙ remote control; Transmitter tab *Modulator* form card
      and status card. Tests: crate units (RS limits, PFT with losses, recordings,
      UDP), end to end a station's MDI recording through the engine, RSCI over UDP
      with PFT/FEC and a lost fragment per packet plus RCI back, RCI remote control,
      the modulator from a recording (receiver decodes it with no audio lost) and from
      UDP with a lost frame filled.
- [x] **Live Journaline updates** (2026-10-02) — GitHub issue #1: the page file
      edited while transmitting reaches the air without a restart.
      *Done:* `JournalineEncoder::replace_all` (decdrm-data) makes the carousel carry
      a new page set atomically (everything encoded first): the same content keeps its
      revision, new content gets the next one (mod 8), pages that are gone stop;
      once the carousel is on the air, new and changed pages are sent next, ahead of
      the cycle (the cycle then goes on where it was). The station keeps a second
      handle to each Journaline carousel (`Arc<Mutex<_>>`, the packet multiplexer
      owns the boxed source) and a `JournalineWatch` per page file: checked once a
      second (modification time and length), loaded once unchanged for a check, so a
      file still being saved is not read; a page file that does not validate changes
      nothing and is reported in the log (once per change).
      `Station::reload_journaline()` loads at once; `AppStatus::journaline` (pages,
      updates, the last error). GUI: the Transmitter tab's services card shows them,
      with an *Update* button; CLI status lines show the pages and updates. Tests:
      carousel units (order, revisions, removal while queued, all or nothing), the
      watch with simulated time, a loopback in which the receiver sees the changed
      menu and page (revision 1) and the new page, the GUI worker's Update path.
- [x] **Audio recording in the GUI** (2026-10-02) — GitHub issue #2 (Dream's *Save
      audio as WAV*).
      *Done:* engine `Command::StartRecording(path)` / `StopRecording`; `AudioOut`'s
      recorder keeps the decoded format (WAV or FLAC by the name, 16-bit), opens at
      once when audio is coming (else with the first audio), carries on in `name-2.wav`
      … (never an existing file) when the format changes, checkpoints every 5 s
      (`FileWriter::flush`: hound rewrites the WAV sizes, so a crash loses at most 5
      s), and ends with its error (logged once) if a write fails; its state
      (`RecordingStatus`: files, seconds, format, active, error) is in the snapshot,
      and the last one stays after it ends. GUI: *Record…* / *Stop recording* under
      the volume (dialog with `<service> <date> <time> UTC.wav` in the last folder),
      the time and file while recording, *Saved … in …* with *Show* afterwards;
      `decdrm-gui --start --record FILE` for timed recordings with `--exit-after`. The
      CLI's `--out` uses the same recorder (numbered parts instead of a time stamp).
      Tests: recorder units (format changes, existing files, errors, waiting for
      audio), the WAV checkpoint read while still being written, the commands end to
      end on a station signal received in real time (the 1 kHz tone in the file).
- [x] **Diversity reception** (2026-10-01) — one station through two KiwiSDRs far
      apart, combined before decoding (the user's choice of next feature; Dream has
      no such thing).
      *Done:* `rx::diversity`: a `DiversityReceiver` of two full receivers in branch
      mode (`ReceiverConfig::diversity_branch`: the chain hands each multiplex frame's
      equalised MSC cells out, `ReceiverEvent::MscCells`, instead of decoding them;
      `MscDecoder` extracted from the chain and shared). The combiner pairs the
      frames carrying the same transmitted frame, maximum-ratio combines them and
      decodes once. Weights: |H|²/σ² per cell, σ² per branch and frame from the
      decision-directed error times |H|² (as the WMER), in two passes, the second
      against the first combination's decisions (a weak branch's noise reads low
      against its own wrong decisions; without the second pass combining with a very
      poor branch came out 2 % worse than the good branch alone). Pairing: frames
      numbered per branch from the input time since the branch's previous frame,
      snapped to the frame's super-frame position (jitter and the Kiwis' clock drift
      do not matter; an absolute time grid flipped numbers near half-way points); the
      second branch anchored by content (hard-decision agreement 6σ above chance,
      1/points); every pair checked the same way, three mismatches or a branch falling
      more than `MAX_LAG` (8) frames behind the decoded ones (input lost: a Kiwi
      reconnection) re-anchor it. Frame n is decoded once both gave it or none can any
      more (went past it, or lags more than `MAX_LAG`); `flush()` at the end of input.
      Engine: `InputSpec::Diversity` (two sources read side by side with a short wait
      each; one ending leaves the other running), `Session::new_diversity`/
      `push_branch`/`flush` (a branch losing sync keeps the multiplex while the other
      holds; status and plots from the branch with the better SNR, the MSC
      constellation from the combined cells), snapshot `diversity` and `input.kiwi2`.
      CLI `--kiwi2`; GUI *2nd KiwiSDR* field, *Find…* context menu, status strip
      (both Kiwis, *Diversity*). Tests: combiner units (pairing by content then time,
      SNR weights, a lone branch, a branch that stops, a stream that skips, another
      station's frames, clock phase and drift), `tests/diversity.rs` (two independently
      fading channels, branch B 0.9 s late on its own clock: channel 3 at 13 dB 92
      frames vs 61 / 32 alone of 100; the ignored sweep over channels 1–4 and 8–20 dB:
      never below the better branch, e.g. AWGN 8 dB 124 vs 0 / 0, channel 4 10 dB 97 vs
      6 / 1, channel 3 12 dB 137 vs 26 / 28 of 150), and two stand-in Kiwis end to end
      through the engine. Live: CNR1 6030 kHz via Mishima and Osaka, 133 frames
      combined, 140 of 145 multiplex frames correct, no audio concealed (Mishima alone a
      minute before: 110 of 132, 20 concealed).
      *Diversity tab* (2026-10-02, the user's request: show how the signals are mixed):
      the combiner records every frame decoded or lost (`MixRecord`: branches, each
      one's SNR = mean |H|²/σ² with the re-estimated noise, the combined SNR, the
      weight share; the last `RECENT_MIX` = 256 in the snapshot, the GUI keeps 10 min)
      and the last combined frame's weights per carrier (`CarrierMix`, from the cell
      map's carrier of each MSC cell, which the chain now hands out with the cells);
      `Receiver::msc_cells` gives each branch's constellation cheaply. GUI tab
      *Diversity* (shown while diversity reception runs): weight-share bar, SNRs and
      gain, counts, pairing; per carrier the SNRs and the share (stacked areas); frame
      by frame the SNRs and share with marks for single-branch and lost frames; the
      constellations of both branches and the combination. Live check on CNR1 via
      Mishima and Osaka: 9.2 + 10.4 dB → 12.9 dB combined, fades of the two Kiwis on
      different carriers (one at −11 dB where the other held 10 dB).
- [x] **Plots at 60 Hz** (2026-10-01) — the plots follow the signal as fast as it
      changes, at the user's request (they updated at 10 Hz).
      *Done:* `EngineConfig::publish_interval` (100 ms by default, so the CLI is
      unchanged; the GUI asks for 1/60 s and reads live input in pieces that short);
      `Engine::snapshot_if_newer` (the GUI polls every frame and copies new snapshots
      only; any change to the shared snapshot bumps `seq`). The chain keeps the last
      frame's FAC/MSC cells and super frame's SDC cells as sliding windows, and the
      channel power per symbol (`chan_rows` with a running `chan_seq`); the input
      spectrum keeps a lightly averaged row per FFT (`waterfall_rows`, `spectrum_seq`).
      Snapshots carry the latest few rows and the GUI appends those it has not seen, so
      none is lost although publishes come in bursts (taking the latest row per
      snapshot got only half of the FFTs). The delay–Doppler map is made when visuals
      are taken and new symbols came (`ChannelHistory::map`), not per symbol (it costs
      ~4 ms). Waterfall and fading map show a fixed 600 rows, so they scroll as fast as
      they update (2.3× and 3.75× the old speed), from ring textures: a new row is one
      partial upload, the image is drawn with its texture coordinates shifted (vertical
      repeat), and rows keep the colours they arrived with. Repaints run at 60 Hz:
      egui starts a requested repaint a predicted frame time early, so that time is
      added back; requesting 1/60 s plainly ran at the display's 165 Hz (90–100 % of a
      core). Measured: 7–9 % of a core for the whole process at 60 Hz, against 3 % at
      10 Hz.
- [x] **KiwiSDR client** (2026-10-01) — DecDRM tunes a KiwiSDR itself and decodes its
      I/Q, instead of a browser tab and a virtual audio cable.
      *Done:* `decdrm-kiwi`. The protocol follows the reference client kiwiclient
      (jks-prv): `SET auth`, `SET AR OK` on `audio_rate`, the receiver set up on
      `sample_rate` (squelch and generator off, `ident_user`, `mod=iq` ±5 kHz at the DRM
      frequency, AGC, compression off), `SET keepalive` once a second; `SND` blocks
      (flags, sequence, S-meter, 10-byte GPS stamp, big-endian I/Q; the first block,
      left over from the channel's previous user, dropped); `load_cfg` gives the name and
      location. Live findings (firmware 1.902, six public Kiwis): kiwiclient's
      `/<ts>/SND` is upgraded but then ignored, so the client opens the web client's
      sound-only type `/no_wf/<ts>/SND` (a channel without a waterfall when free), with
      `/<ts>/SND` as the fallback after 5 s of silence; the kiwisdr.com proxy redirects
      with `HTTP/1.0 307` to `<id>.proxy2.kiwisdr.com` (port 80), which tungstenite's
      handshake rejects, so the upgrade handshake is our own (accept key checked) and
      tungstenite only frames; owners can cap the channels of "apps" (clients that
      never loaded the web page): such a Kiwi admits DecDRM and sends `too_busy=N` a few
      seconds later. Policy: nothing that refuses (busy, password, app cap, down, an
      offline proxied Kiwi) or closes the session cleanly (time limits) is retried, nor
      a Kiwi that never streamed; a connection lost after streaming is retried with
      backoff (2 s doubling to 30 s, 10 in a row); HTTP and Kiwi redirections followed;
      no pretending to be a browser. `KiwiStream` runs the connection on its own thread
      with a 30 s FIFO; stopping shuts the socket down from the caller, so a slow Kiwi
      never holds up the engine or the GUI. Engine `InputSpec::Kiwi` (I/Q forced, the
      resampler from the reported rate, Kiwi log and status in the snapshot), CLI
      `decdrm rx --kiwi`, GUI *KiwiSDR* source (address with recent ones, frequency,
      name, password kept for the run), status strip (state, S-meter, name), *Find a
      KiwiSDR* (the public list from rx.linkfanel.net, downloaded on request; filtered
      to Kiwis that allow apps, have a free channel and receive the frequency), and a
      double-click in the *Schedule* tab. Tests: protocol and address units, the client
      against `mock::MockKiwi` (bit-exact I/Q, set-up commands, keepalive rate,
      refusals, reconnection, clean close, HTTP/1.0 and Kiwi redirections, path
      fallback, stop within 1 s), and a station's I/Q decoded end to end through the
      stand-in (205 audio frames, none concealed). Live: KCBS on 6140 kHz locked through
      Kiwis in Japan (25.8 dB SNR, every 16-QAM MSC frame correct, at a Kiwi that then
      applied its app cap) and streamed 60 s without interruption elsewhere.
      *Retuning* (2026-10-01): `KiwiStream::tune` sends `SET mod=iq … freq=` on the
      open connection (as the Kiwi's web page and kiwirecorder's scanning do), and a
      reconnection sets up the new frequency. Unread samples are discarded, and blocks
      are dropped until `RETUNE_SETTLE` (1 s) after the command. On the Mishima Kiwi the
      S-meter followed 0.3–0.5 s after it, so 0.5 s was too tight. Engine
      `Command::Tune` restarts the receiver as a new station (the selection and the
      frame counts go too). In the GUI the frequency box stays live while a Kiwi runs:
      the other source controls are locked one by one, so the row wraps as before. A
      typed value is applied on Enter and a drag on release, and a *Schedule*
      double-click retunes the running Kiwi. Tests: the retune on the open connection
      (no samples before the settling time), a reconnection keeping the new frequency,
      and end to end (the station found and decoded again on the same connection).
      Live: KCBS 6140 → CNR1 6030 kHz on one connection; CNR1 found 1.9 s after the
      retune, xHE-AAC audio from 4.3 s.
- [x] **Web stream input** (2026-10-01) — the transmitter relays an internet radio
      stream: `[service.audio.input] url = "…"`, with `stream_titles` (default on).
      *Done:* `decdrm_station::webstream`. HTTP/1.1 client of our own (SHOUTCAST's
      `ICY 200 OK`, chunked coding, redirects, reads that poll a stop flag and give up
      after a stall; rustls with the ring provider and the system's roots); M3U/M3U8/PLS
      resolve to their first entry (relative entries, dot segments), HLS refused. ICY
      metadata stripped, `StreamTitle` parsed (apostrophes, Latin-1). Format by sniffing
      (two agreeing frame headers), then `Content-Type`: MP3/MP2 (own framer, symphonia
      decoder), AAC/HE-AAC/HE-AAC v2 in ADTS (`decdrm_codecs::FdkAdtsDecoder`; PS known
      by FDK's flag, as FDK delivers mono HE-AAC as stereo), Ogg Vorbis (symphonia), Ogg
      Opus (libopus; pre-skip, output gain), FLAC native (CRC-16 framing; a synthetic
      STREAMINFO when joined mid-way) and in Ogg. Our Ogg demuxer follows Icecast's
      chained streams and takes titles from their comments (symphonia's stops at a
      chain). A worker thread decodes, mixes/resamples to the encoder and fills a FIFO,
      waiting when it is full. Sound-card output: the sound-card input's drift loop,
      shared as `ClockFollower`, with a network tuning (1.5 s backlog, 250 ms → 1000 ppm,
      20 s filter: ±0.2 s of jitter → < 200 ppm ripple, simulated); the first read lets
      the server's connection burst arrive and drops it beyond the start-up backlog
      (real time: 1.51 ± 0.02 s); re-buffering after an underrun, skipping beyond 2 s
      over the target; a response with a length (a file) is neither trimmed nor
      skipped. File output: the stream paces the station. Losses → silence and
      reconnection with backoff 1–30 s; the first connection's failure is the station's
      error. Titles are queued with their FIFO position and go on air with their audio,
      first in the text message cycle (`TextMessageEncoder::set_messages`: segment
      boundary, toggle bit inverted). Status `WebStreamStatus` (state, codec, bit rate,
      rate, channels, station name, genre, title, buffer, reconnects, underruns,
      errors), log via `Station::take_log` (printed by `decdrm tx`). Tests against
      servers on 127.0.0.1 with streams made from tones (FDK ADTS encoder, minimal MP3
      and Vorbis writers coding one MDCT line, libopus + Ogg muxer, flacenc), TLS with a
      test CA, playlists, redirects, dropped connections, errors, pacing, and a station
      loopback through the receiver; live through the virtual cable: 80 s, 0 underruns,
      titles received. Not verified: real Icecast/SHOUTCAST servers and real encoders'
      streams (no internet in development).

- [ ] **DRM+ / robustness mode E** (started 2026-10-23) — mode E is the VHF DRM+ mode:
  213 carriers at a 444 4/9 Hz spacing reaching ±47.11 kHz, on the 100 kHz raster, with
  **100 ms** transmission frames of 40 OFDM symbols, 244 FAC cells over symbols 5..=26,
  no unused carriers, no frequency reference cells, and a two-service 116-bit FAC at code
  rate 0.25. It needs a **120 or 192 kHz** working sample rate, which is why `WorkingRate`
  now exists and is carried by the transmitter and receiver.

  *Transmit: works.* A station with `mode = "E"`, `occupancy = 0` and
  `working_sample_rate = "192"` validates and transmits, and the signal it writes measures
  as real DRM+ (Tu 432 samples, Tg 48, span ±47.29 kHz) by the same analysis used on the
  reference recordings. Mode E at 48 or 96 kHz is refused by `check_mode_rate`.
  `tests/mode_e_tx.rs` asserts the signal against those measurements.

  *Receive: works* (2026-10-26). Every DRM+ recording in `refs/` decodes end to end —
  FAC, SDC, multiplex frames and **xHE-AAC audio** — at both rates and down to 3 dB SNR.
  The two mode E loopback tests in `tests/mode_e_rx.rs` that used to be ignored-passing or
  failing now pass unignored. Two bugs had to be found
  for that: the MSC interleaver depth is **D = 6** for mode E (§7.6), not the 5 the code
  hardcoded, and the channel estimator's timing rotation must use the FFT size *at the
  working rate* (it used the 48 kHz one, 4× too small at 192 kHz). A third, in the PDS
  tracker's `ir_per_sample`, was the same class of mistake.

  **The 120 kHz fault is solved by upsampling to 192 kHz** (2026-10-25). `examples/upsprobe.rs`
  generates mode E at 120 kHz, converts it to 192 with `channel::resample::Resampler`
  (ratio 1.6), and decodes at 192: **FAC 193, SDC 46 over 20 s**, against FAC 193, SDC 47
  for the same signal generated and decoded natively at 192 — i.e. identical, where the
  120 kHz receiver alone gives FAC 3, SDC 0. This is the intended design, not a workaround:
  **JvanKatwijk's DRM+ decoder — which decodes these recordings — runs entirely at 192 kHz**
  (`#define SAMPLE_RATE 192000`, a 192000-entry mixer table, and no input resampler at all).
  It does not have a 120 kHz path. Mode E's 1.9 kHz carrier spacing over ±47.11 kHz is
  comfortably inside 192 kHz's 96 kHz Nyquist, and the upsampling image of the band edge
  (72.89 kHz) is 23 kHz clear of it, so the interpolation filter removes it.
  **The consequence for DecDRM: convert 120 kHz input to 192 kHz and receive everything at
  192.** A file at 120 kHz needs `rate × 1.6`; a live 120 kHz SDR needs the same. The native
  120 kHz receive path can then be left alone — or removed — rather than debugged further.

  Still failing, in decreasing order of how well it is understood:

  1. **Real DRM+ recordings did not decode — fixed** (2026-10-25 and 2026-10-26). All
     eleven 192 kHz DRM+ recordings in `refs/` now decode end to end, audio included:
     `decdrm rx <file> --drm-plus --format iq --dc 20000` — the recordings carry their
     carrier at +20 kHz, and `--dc` tells the pinned mixer where it is (`ReceiverConfig::
     dc_hz`; the GUI's receiver options show the same field when a mode is pinned). Before
     that field existed the carrier had to be mixed down by hand first (`--example mixwav`),
     which is what `refs/m20k_20k_offset_4QAM_LPP0_192kHz.wav` is: the 4-QAM LPP0 recording
     mixed down by +20 kHz, kept as a baseband copy for tools that cannot mix. It decodes
     without `--dc`: FAC 310, SDC 77, MSC 302:

     | recording | MSC frames (ok) | audio frames ok |
     |---|---|---|
     | 4-QAM LPP0, 1 service | 307 (151) | 566 |
     | 4-QAM LPP1 | 175 (85) | 398 |
     | 4-QAM LPP2 | 271 (133) | 623 |
     | 4-QAM LPP3 | 499 (247) | 1157 |
     | 4-QAM LPP0, 3 dB AWGN | 283 (139) | 521 |
     | 4-QAM LPP0, 6 dB AWGN | 283 (139) | 521 |
     | 16-QAM LPP0, 1 service | 511 (253) | 1185 |
     | 16-QAM LPP1 | 331 (163) | 1528 |
     | 16-QAM LPP2 | 343 (169) | 1584 |
     | 16-QAM LPP3 | 307 (150) | 1414 |
     | 16-QAM LPP3, 4 services (2 xHE + 2 HE) | 535 (265) | 994 |

     (About half the multiplex frames are counted before the long interleaver has filled,
     which is the expected `D - 1` of them; the concealment count is 0-1 per file.)

     Three further faults had to be found for the **audio** to come out, each invisible in
     a DecDRM-to-DecDRM loopback because both ends made the same mistake:

     - **The guard-correlation step was coarser than mode E's guard interval.** `timesync`
       searches for the symbol boundary every `STEP` = 4 *decimated* samples; at 192 kHz
       the decimation is 16, so that is 64 real samples, while mode E's guard is only **48**
       (3 decimated). The correlation peak fell between grid points and the loop parked the
       FFT window ~280 samples out — outside the guard, where the cyclic prefix no longer
       covers the window and every data cell is wrong. Only the heavily protected FAC
       survived, which is why this read as a signal-quality problem for so long, and why
       whether it decoded at all depended on the transmit filter's group delay.
       `timesync::step_for` now takes the fine step whenever `STEP` is wider than the mode's
       decimated guard. *Measured: removing the transmit band-limiting filter had moved the
       counts before (FAC 3 to 66); with this fix they rise at both rates without it
       (FAC 77 of 77, SDC 19, MSC 75, bit-exact on a synthetic signal).*
     - **The receiver's MSC decoder hardcoded mode B's protection tables.** `MlcParams::msc`
       is fixed to `RobustnessMode::B`, and `rx/mscdec.rs` (and `tx/mod.rs`) called it, so
       mode E's 16-QAM used table 30 instead of table 31 and its 4-QAM had no rates at all.
       The rates differ at every level **except the last**, where both tables say 0.62 —
       which is why 16-QAM LPP3 decoded while LPP0/1/2 came out as noise, and why the fault
       looked like an equaliser problem. Both sides now call `MlcParams::msc_for(mode, ...)`.
     - **The audio super frame spans two logical frames in mode E.** §5.3.1.0 and clause 6:
       "For robustness mode E, the audio super frame is mapped onto two logical frames,
       since the audio super frame is of twice the duration of the logical frame" — 200 ms
       of xHE-AAC over two 100 ms multiplex frames. `AudioDeframer` was fed one logical frame
       per multiplex frame as for A-D, so every xHE-AAC super frame was parsed from half its
       bytes and failed. The two halves are not aligned to anything in the bits, so
       `AudioDeframer::new_at(params, stream, mode)` now collects `super_frames_per_logical`
       parts and finds the boundary by the super frame's own check — the fixed-header CRC-8
       plus every directory element repeating the border count (§5.3.1.1-2), the same way the
       rest of the stream is found.

     The four coding faults fixed on 2026-10-25 were responsible for the FAC/SDC half, all in
     the same place: **every field that the FAC's RM flag re-interprets was being read with
     the RM-flag-0 table.** §6.3.3 says "the interpretation of [the interleaver depth, MSC
     mode, SDC mode and toggle] is dependent on the value of the RM flag"; DecDRM read one
     table for both modes. Each fault below was confirmed by reverting just that change and
     watching the count collapse:

     - **The occupancy field was read 4 bits wide.** §6.3.3's channel parameters are 20 bits
       in *every* mode — occupancy is **3** bits — but the code read 4, swallowing the RM
       flag as the value's top bit. Mode E's `1 000` became 8, not a valid occupancy, so
       every real mode E frame was rejected (`FAC 0`). DecDRM's own transmitter wrote the
       same wrong width, so only a loopback ever passed. *Reverting to the 4-bit read leaves
       FAC at 1269 but MSC at 0.*
     - **The MSC-mode field used the A–D table.** RM flag = 0 codes `00` 64-QAM / `11`
       16-QAM; **RM flag = 1 codes `00` 16-QAM / `11` 4-QAM**. Reading the A–D table reports
       each mode E signal one step high — a real 4-QAM file as 16-QAM, a real 16-QAM one as
       64-QAM — and there was no 4-QAM MSC variant at all. `MscMode::from_bits` now takes the
       flag and can return `None` for the reserved mode E codes.
     - **The SDC-mode bit was treated as a constellation only.** In modes A–D it picks 16-QAM
       or 4-QAM; in **mode E it is the SDC protection level** — `0` is 4-QAM at rate 1/2, `1`
       at rate **1/4** (table 38). `MlcParams::sdc_for` was called with a hardcoded level 0 in
       both places that build an SDC decoder, so a mode E SDC sent at rate 1/4 was decoded
       with the wrong code rate and the CRC never passed. *Reverting this drops SDC to 0 and
       MSC to 0.* `SdcMode::Qam4Rate25` carries the level, and `SdcMode::protection()` feeds
       it through.
     - **The Identity field was read as the frame index.** It encodes *position in the FAC
       cycle*: `00`/`11` first, `01` intermediate, `10` last. Modes A–D have three frames and
       one intermediate block, so identity == frame index works; **mode E has four frames and
       two intermediate blocks, both `01`, told apart by the toggle flag** (§6.3.3: "zero for
       the first and third FAC block and to one for the second and fourth"). The real signal
       sends `{0|3} 01 01 10` with toggle `0 1 0 1`. Mapping identity straight through
       mis-numbered frames 2 and 3 as 1 and 2, so the super frame restarted constantly and
       the multiplex frame was never assembled. *Reverting this leaves FAC 1269 and SDC 316
       but MSC 0* — the SDC gate is not the only consumer, the MSC assembler needs the count
       to reach 3. `frame_index_of` now takes the toggle and the frame count, and
       `identity_and_toggle` is its inverse for the transmitter.

     `examples/modee_real.rs`'s earlier "FAC 0/681" reading was a symptom of the occupancy
     width, not a demodulation fault: that tool has **no equaliser**, so its failure was never
     evidence about the receiver's decoding.
  2. **A pinned mode did not establish its own frame alignment — fixed.** The file round trip
     looked like a signal problem (band limiting on gave FAC 0, off gave FAC 2-3) but is a
     timing one. Measured on the written files: the two are the **same signal shifted by ~415
     samples** (the transmit filter's ~697-tap group delay), agreeing to **0.36 % of peak**
     once aligned, so nothing is corrupted. The cause was one line: `Receiver::set_mode` forced
     `state = Tracking`, which skips the frame-sync attempts *and* the no-FAC resync, so a
     pinned signal only decoded if it was already aligned — true of a signal generated in the
     same process, false of any file. Staying in `Acquisition` took a written-then-read mode E
     file from **FAC 3 to FAC 63** (7.5 s, 192 kHz), so a mode E file now decodes in practice.
     Pinning says where the signal is, not where the frame boundary is.
     *Band limiting, now fully explained:* the filtered file carries **412 leading zero samples**
     (the FIR's group delay, written as silence) that the unfiltered one does not, so the
     receiver counts frames from before the signal starts. Stripping those 412 samples gives
     **FAC 72, FAC bad 0** - every frame decodes - which confirms the waveform was never the
     problem and the whole difference is that lead-in. The **real remaining bug** is therefore
     the receiver's tolerance for a fractional-symbol lead-in: it should resolve a sub-symbol
     offset at the start of a signal, and does not. (Or `OutputStage` should not emit the
     filter's warm-up: it reports `delay()` but nothing consumes it.)
     Where the lead-in should be fixed, and a constraint on doing it: `OutputStage::process`
     deliberately returns input + 2·`latency()` samples and the tail is drained at the end by
     `OutputStage::flush` - so the filter's **warm-up at the start** is handled symmetrically
     nowhere, and the station writes it into the file as silence. Two of `output.rs`'s own tests
     pin that length (they skip `2 * latency()` by hand), so dropping the lead-in inside
     `process` would break a deliberate invariant. The fix belongs either in the station (drop
     the first `latency()` samples of the first frame it emits) or in the receiver (tolerate a
     sub-symbol lead-in) - the first is smaller and symmetric with the existing tail handling.
     *The lead-in bug is fixed* (2026-10-25). The cause was in `Receiver::restart`: it re-armed
     the 48 kHz frequency scan unconditionally, even for a receiver whose mode was pinned. A
     pinned receiver runs at DC with no scan; after its first restart it began hunting for a
     DRM30 signal it could never find and never came back to the signal at DC. `restart` now
     leaves a pinned receiver at DC (and keeps mode detection off, since the FAC cannot
     express mode E). Every lead-in value the test sweeps now decodes, where before only 0 and
     5000 did. `mode_e_rx.rs`'s lead test asserts the fix instead of pinning the failure.
     *Still open:* the receiver resolves a sub-symbol lead-in the slow way (one restart, then
     re-acquire) rather than aligning first time; not emitting the transmit filter's warm-up
     would avoid the restart, but the receiver now survives it.

  3. **120 kHz reception — fixed** (2026-10-26), by the same guard-step fix as item 1: at
     120 kHz the decimation is 10, so mode E's guard is 5 decimated samples against a
     `STEP` of 4. `tests/mode_e_rx.rs::mode_e_loopback_at_120k` (which used to carry a long
     list of measurements ruling out the equaliser) now passes unignored. The upsampling
     note above is therefore no longer needed as a workaround, though it remains a valid
     design choice — JvanKatwijk's decoder runs at 192 kHz only.
  4. **`freqacq.rs` finds mode E, and the guard interval tracks its carrier** (2026-10-26,
     completed 2026-11-03), so a DRM+ signal need not be pinned. The pilot detector cannot
     find it — it hunts for the three DRM30 frequency reference pilots at 750/2250/3000 Hz,
     and mode E defines none (§8.4.2) — so `FreqAcquisition::detect_plateau` searches for
     the signal's **spectrum plateau** instead: the window of the band's own width (213
     carriers, ±47.11 kHz) whose mean power most exceeds the mean outside it. That is a
     matched filter, so it needs only that the band be *relatively* stronger than its
     surroundings — no absolute threshold, which an edge test cannot avoid and which made
     the noisiest recordings unfindable. It runs only above 48 kHz, where a 95 kHz DRM30
     signal cannot exist, and the FFT length, hop and analysis span scale with the working
     rate, keeping the 7.8 Hz bin the threshold was set for.

     Two things the unpinned path needed besides the detector: an unpinned receiver that
     mode detection has switched to E must build its chain at **occupancy 0** (mode E has
     no other, §8.1 — the DRM30 default leaves `SymbolChain::new` returning `None` and the
     receiver stuck in acquisition with no chain), and `mode_e_rx.rs` guards the whole
     thing (`mode_e_is_acquired_without_pinning`, with the signal put off centre on
     purpose).

     **The carrier is what was missing** (2026-11-03), and it is found in two stages. The
     ratio's peak is flat — the window is as wide as the band — so the crest centroid that
     first located it wandered: measured over ten 0.47 s windows of each reference
     recording, 14–62 Hz of standard deviation, and up to 220 Hz between the receiver's own
     re-acquisitions. A band **edge** is far better conditioned, so the centre now comes
     from an antisymmetric kernel (the derivative of a Gaussian) on the spectrum smoothed
     by one carrier spacing: each edge becomes an extremum, a parabola puts it at a fraction
     of a bin, and the two are averaged — which also cancels a symmetric transmit-filter
     rolloff. Measured the same way: **4.5–14 Hz, bias under 5 Hz**. Then
     `Receiver::track_cp_frequency` refines it in the time domain: mode E has no pilots, but
     the guard interval is a copy of the symbol's tail, and the phase of its correlation is
     `−2π·Δf·Tu` — unambiguous within ±222 Hz (Tu = 2.25 ms), independent of the channel
     (both samples pass through the same taps), and averaged over 16 symbols (40 ms) down to
     about a hertz.

     **Measured.** Shifting a pinned receiver's mixer over a reference recording: the FAC
     and MSC decode fully within **±20 Hz** of the carrier, partially at ±50 Hz (30–54 FAC
     of 311), and not at all at ±100 Hz. With both stages **all fourteen recordings decode
     unpinned**, within two multiplex frames of their pinned counts and without
     re-acquiring; before, two did, and the receiver restarted every four seconds with a
     fresh (bad) estimate. The pinned path gets the same refinement, so `--dc` only has to
     be within a few tens of Hz.

     *Reachable from the GUI and CLI now* (2026-10-25): `ReceiverConfig::pinned` carries a
     mode/occupancy to decode instead of searching, so `Receiver::new` pins it without a
     caller having to reach in with `set_mode`. The GUI's source bar has a **mode picker**
     (Auto / Mode E (DRM+), which also raises a working rate that cannot carry the mode);
     the CLI has `--sample-rate` for the rate and `--drm-plus` to pin the mode. Mode E's
     occupancy is always 0 (§8.1), so the picker derives that rather than offering a choice.
     **Auto works now too** (see item 4): the mode picker is a shortcut, not a requirement,
     and a DRM+ signal no longer has to be tuned to the receiver's centre frequency.

     **The working rate follows the input** (2026-11-03). It is a property of the recording,
     not a choice: demodulating at another rate means resampling, and a 192 kHz DRM+ file
     resampled to 48 kHz is gone. Opening a recording in the GUI now sets the rate picker to
     the file's own rate, and `decdrm rx` does the same when `--sample-rate` is left out, both
     through `WorkingRate::closest` (nearest supported rate, 48/96/120/192; 44.1 kHz DRM30
     stays at 48). A sound card or a KiwiSDR has no file to ask, so those keep 48 kHz unless
     the caller says otherwise.

     **A quiet lead-in above 48 kHz used to be "detected" as mode E** (2026-11-09). Silence
     scores exactly zero in every mode, and `decide` compared the best score with the
     second-best as a *ratio* — 0/0 = ∞, an infinitely certain detection — while `max_by`
     returns the **last** maximum on a tie, which among the five modes is mode E. So a stretch
     of digital silence decided DRM+, and the receiver switched to mode E, built its chain for
     a signal that is not there, and only got back to the right mode after a restart (measured
     at 192 kHz, mode A behind 0.5 s of silence: mode detection reported `[E, A]`, and the
     acquisition it threw away costs the first seconds of the recording).
     The window is a coincidence of two lengths: `freqacq` hands time sync the ~0.45 s of
     samples it buffered before acquiring, and mode detection needs `RM_BLOCKS` = 16 symbols
     before it decides — **41 ms** for mode E's 100 ms frames, but 0.43 s for mode B's 400 ms
     ones. Only above 48 kHz is the mode E window short enough to fire inside the pre-signal
     buffer, which is why this never showed in the 48 kHz loopback. `decide` now requires a
     score above `MIN_MODE_SCORE` (1e-3) before it compares anything: a clean mode A scores
     ~0.08 and the modes that are not on the air ~0.005, so the floor is two orders of
     magnitude below a real detection. The same floor makes the all-zero case at 48 kHz
     (where mode D is the last maximum) impossible too.

     That bug was found while looking at DRM30 at 192 kHz, and the look turned up a **test
     gap rather than more faults**: `loopback.rs` runs every layout at the 48 kHz default and
     `mode_e_rx.rs` only mode E, so nothing exercised modes A–D at another rate. The new
     `tests/modes_at_rates.rs` decodes all four DRM30 modes, unpinned, at **48, 96, 120 and
     192 kHz** (FAC ≥ 8 of 15 frames each, and the detected mode is the transmitted one), and
     adds the lead-in regression: four lead lengths of digital silence at 192 kHz — 0.5–0.55 s
     is where the old code reported mode E — plus a noise-floor lead at 120 and 192 kHz for
     every mode. Verified the other way round as well: with the floor removed the lead-in test
     fails at 0.5 s with `detected [E, A]`, and passes with it.

  The `TimeWiener` `params(E)` filter length/Doppler pair and `freq_wiener_len(E)` were
  **placeholders** copied from mode C/D. They are tuned now (2026-11-02): **mode E is
  (3 pilots, σ = 12 Hz)** and the frequency length stays 13, which a 7–15 tap sweep could not
  distinguish. The measurement is in the VHF milestone at the end of this document: the
  placeholder decoded no multiplex frames at 200 km/h, the tuned weights decode 66 of 80.
  `ChanEstTuning` (a `ReceiverConfig` field) now carries overrides so a sweep can repeat this
  without recompiling; `examples/modee_sweep.rs` is that sweep.

  *Fixed in the form since the list above was written:* the sample-rate value was written to
  the TOML as a number where `RateSetting` needs a string (the saved file could not be read
  back, so the choice silently did not apply); the multiplex bar computed kbit/s with a
  hardcoded 400 ms frame, so mode E showed 4× too low; mode E showed a one-item bandwidth
  selector instead of the fixed 100 kHz it is defined at. The channel rows now also follow the
  mode's own rules — MSC 16-QAM or 4-QAM, SDC 4-QAM, long interleaving, no hierarchical
  layer — greying out what the mode removes and repairing a document that holds one, since
  the station's own validation accepts those. See "What robustness mode E changes in the rest
  of the form" in `tx_form.rs`.

  *Fixed in the receiver's displays* (2026-10-26): the three readouts that come from the
  channel parameters were all wrong for mode E, and all three for the same reason — each
  read a mode E value with a DRM30 table, exactly as the FAC parser had. §6.3.3 says the
  spectrum-occupancy parameter's interpretation "is dependent on the value of the RM flag",
  and table 48 gives value `0` two meanings: 4.5 kHz for modes A–D and **100 kHz** for mode E.
  `SpectrumOccupancy::bandwidth_khz` had no mode argument, so it always answered 4.5. Each
  fault is measured against the pre-fix binary on the same 4-QAM LPP0 recording pinned with
  `--drm-plus --format iq --dc 20000`, whose decoding is identical before and after (FAC 309,
  SDC 77, MSC 307, 566 audio frames, 0 concealed):

  - **The bandwidth read 4.5 kHz.** The CLI's status line and the GUI's "BW" field printed
    `SO0 (4.5 kHz)`; they now print `SO0 (100 kHz)`. The mode now reaches the bandwidth
    through `bandwidth_khz(mode)` and `ChannelLayout::bandwidth_khz()`, and
    `SpectrumOccupancy`'s own `Display` names only the parameter (`SO0`) since it cannot know
    the mode. Callers updated: the CLI status line, the GUI status strip, the CSV logger's
    `bandwidth_khz` column, and the station's plan/modulator summaries.
  - **The SNR read 13.5 dB too high** — 54.4 dB instead of 40.9 dB on that recording. The
    channel estimator refers its SNR from the system bandwidth to the nominal one
    (`sys_to_nom_bw`); with the DRM30 reading the ratio is 94 667/4 500 ≈ 21× (+13.2 dB)
    where it should be 94 667/100 000 ≈ 0.95 (−0.24 dB), so the printed figure was inflated
    by 10·log10(100 000/4 500) = **13.47 dB**. Measured on the same signal: 53.7/54.4/54.4 dB
    before, 40.3/40.8/40.9 dB after — the same 13.5 dB drop at every status line, while MER,
    WMER, Doppler and the FAC/SDC/MSC counts are unchanged. That field feeds only
    `stats.snr_db` and the per-carrier SNR plot, never the equaliser or any decision, so this
    is a display correction and cannot move a decode.
  - **The bit rate read 4× too low.** `stream_bitrate` divided by a hardcoded 400 ms, but
    §8.1 gives mode E **100 ms** logical frames ("the logical frames are each 100 ms long";
    its 200 ms audio super frame spans two of them). The duration now comes from
    `RobustnessMode::frame_ms()`, so DRM+ streams report 4× the old figure. The transmit
    side's multiplex bar already had this fix; this is the receive side's `stream_bitrate`,
    which feeds `ServiceView::audio_bitrate` and each application's `stream_bitrate` — the
    service bars in the GUI. *Verified end to end:* the recording delivers 307 multiplex
    frames in 31.2 s, i.e. 101.6 ms per frame, confirming the 100 ms geometry.

  The receive-side and transmit-side fixes are the same three misreadings, so a DRM+ station
  now agrees with the receiver that decodes it. Not changed: `Source::position_s` keeps its
  400 ms for MDI input — MDI's `robm` field is 2 bits (§ "robm", 0–3 = A–D) and cannot carry
  mode E, so a 400 ms frame there is correct.

  *Both of mode E's constellations now encode, at all four of its levels each* (2026-10-26).
  The transmitter capped 16-QAM at level 1 in every mode (the DRM30 table 30 cap), and the
  form hid 4-QAM for mode E on the grounds that its rates were not encodable. §7.5.1 gives
  mode E four levels of each — table 31 for 16-QAM (0.33, 0.41, 0.5, 0.62) and table 29 for
  4-QAM (0.25, 0.33, 0.4, 0.5) — and mode E's tables are in `tables`, so both sides only had
  to stop assuming DRM30's. The cap is now the mode's: 0–1 for 16-QAM in A–D, 0–3 in E, and a
  DRM30 station still refuses level 2 (asserted in `tests/mode_e_tx.rs`). The eight
  constellation/level pairs are each checked against annex J table J.5's MSC input bits per
  frame — 3 727 to 18 635.

  Still open: the transmit side underruns badly at 192 kHz (4× the samples of 48 kHz).

  **The receive side has a rate selector now** (2026-10-24). `ReceiverConfig::working_rate`
  reaches the source: `Source::open_at(spec, rate_hz)` resamples the input to the receiver's
  rate instead of a fixed 48 kHz (`decdrm_io::ToRate`, with `To48k` kept as its 48 kHz case),
  and the GUI's source bar and the CLI's `--sample-rate` set it. The engine's monitor audio and
  `Session`'s time/sample arithmetic follow the same rate. The spectrum plots no longer assume
  48 kHz either: `Visuals::spectrum_span_hz` and the transmitter's `TxSnapshot::output_rate_hz`
  carry the real span, and `Waterfall` derives its frequency axis from it — the bins of a
  192 kHz DRM+ receiver are now drawn where they belong rather than on a 48 kHz axis. A rate
  change restarts the waterfall's history (the axis and bin width change with it).

- [ ] **VHF multipath simulation** (started 2026-11-01) — the channel simulator ran the
  HF models of ES 201 980 annex B at a fixed 48 kHz. DRM+ runs at 120/192 kHz on VHF
  (Band I/II/III), where the impairments are different in kind: its guard interval is
  **250 µs** (`Tg = 3T = 250 µs`), so COST 207's rural and urban profiles sit *deep*
  inside it and the interesting echoes are the long ones (an SFN, a mountain), and a car
  sees Doppler shifts an order of magnitude above annex B's — 9.3 Hz at 100 km/h on
  100 MHz, 18.5 Hz at 200 MHz.

  *Rate-aware, with true sub-sample delays.* `ChannelSimulator::new_at(layout, rate, cfg)`
  converts the paths, the Doppler steps and the noise reference at the working rate;
  `new` stays the 48 kHz case for A–D. The SNR definition is unchanged (in the nominal
  channel bandwidth, which for mode E is the 100 kHz of table 48), so at 192 kHz the noise
  power is `P·192 000/(SNR·100 000)` and the system bandwidth is 213·192 000/432 =
  94.67 kHz. Delays are **not** rounded to whole samples: at 192 kHz COST 207's taps are
  sub-sample (0.2 µs = 0.038 samples), so each path adds a 16-tap Hamming-windowed sinc
  interpolation and the model carries a fixed 7-sample latency, keeping relative delays
  exact. Rounding would merge TU6's six taps into a single one-sample echo and move every
  null — `path_delays_exact` measures the real thing: 0, 0.0384, 0.096, 0.3072, 0.4416,
  0.96 samples.

  *VHF models beside annex B's.* `channel::vhf` adds COST 207 **RA** (rural, 4 taps to
  0.6 µs), **TU6** (typical urban, 6 taps to 5 µs) and **HT** (hilly terrain, 8 taps in two
  clusters out to 17.2 µs), each tap fading with a *classical (Jakes)* Doppler spectrum at
  `f_d = v·f_c/c`; `speed_kmh = 0` gives the static case (multipath, no fading). Jakes
  fading is a 32-sinusoid random-angle sum — a fixed angle grid repeats `cos θ`, whose
  equal-frequency sinusoids beat instead of averaging and biased the tap power by 17 %
  until the angles were drawn at random. `sfn_echo(delay, level)` models the case COST 207
  does not: one echo, possibly beyond the guard. BU (bad urban, twelve taps) is left out
  until its table is checked against the source. Even HT's 17.2 µs is 7 % of the guard, so
  every COST 207 profile is flat as far as mode E's ISI is concerned — what they cost is
  channel-estimation accuracy, not inter-symbol interference.

  Verified by 24 unit tests in `channel::`: Jakes unit power and `J₀(2π f_d τ)`
  autocorrelation, mode E noise and system bandwidth at 192 kHz, delay scaling at 120 and
  192 kHz, a sub-sample delay passing a 3 kHz tone to 2·10⁻², HT's second cluster keeping
  its 2.88–3.30-sample delays, TU6 fading in motion and not when parked, and the frequency
  response of a static two-path channel at 192 kHz.

  *End to end: mode E decodes through a moving VHF channel* (2026-11-01, retuned 2026-11-02).
  `mode_e_rx.rs`'s `mode_e_decodes_through_a_moving_vhf_channel` and
  `mode_e_survives_200_kmh` send 8 s of mode E (80 frames) at 192 kHz through TU6 at 200 MHz
  and decode it. Measured with the tuned estimator, FAC / SDC / MSC frames of 80:

  | channel | FAC | SDC | MSC |
  |---|---|---|---|
  | TU6, parked, 20 dB | 78 | 18 | 66 |
  | TU6, 100 km/h (18.5 Hz), 20 dB | 78 | 18 | 66 |
  | TU6, 200 km/h (37 Hz), 20 dB | 78 | 16 | 66 |
  | TU6, 100 km/h, 10 dB | 77 | 17 | 66 |

  ***The tuning pass*** (2026-11-02). With the C/D placeholders (9 pilots, σ = 2.85 Hz) the
  same runs gave **38/6/48** at 100 km/h and **12/2/6** at 200 km/h: the limit was the
  estimator, not the noise, because doubling the speed collapsed the counts while dropping
  10 dB barely moved them. The truth is that 37 Hz is simply outside what a filter sized for
  a few hertz can follow. `examples/modee_sweep.rs` sweeps the TimeWiener's length (3–9
  pilots) and maximum Doppler (4–12 Hz) over TU6 at 100 and 200 km/h, then checks the winner
  parked and at 10 dB (where a wide Doppler setting could cost instead of gain).

  **Result: mode E is 3 pilots, σ = 12 Hz** (`params(E)`), and the frequency length stays 13 —
  a 7–15 tap sweep could not distinguish it, matching the earlier "not the blocker" note. The
  short filter is what tracks a 37 Hz fade; 12 Hz is the widest the sweep tried and still
  improved, so a faster receiver or a harsher profile should extend the grid rather than
  assume this is the optimum. Parked reception is not a trade: 78 FAC against the
  placeholder's 77.

  `ChanEstTuning` (a `ReceiverConfig` field) carries the overrides the sweep needs, so this
  can be repeated without recompiling the receiver.

  *Reachable from the station* (2026-11-01). The simulator already lived in `[simulate]`,
  with annex B's HF presets; it takes a VHF preset too now — `vhf = "ra" | "tu6" | "ht"` with
  `speed_kmh` and `carrier_mhz` — so a DRM+ station can transmit through a car or a hilly
  profile. Two bugs had to go with it:

  - The station built the simulator with `ChannelSimulator::new`, the 48 kHz entry point. A
    mode E station runs at 192 kHz, so delays, Doppler steps and the noise power were all
    scaled for the wrong rate; it now uses `new_at` with the plan's working rate.
  - `StationStatus::seconds` and the SDC clock counted 400 ms per frame whatever the mode, so
    a DRM+ transmission reported four times its length — 2 s of signal printed as 8 s.
    `MultiplexPlan::frame_seconds()` now takes the duration from the mode.

  Measured: `drm_plus_transmits_through_a_vhf_channel` sends 2 s of mode E at 192 kHz through
  TU6 at 100 km/h on 200 MHz at 20 dB, and the file and status carry the right rate and time.
  128 of ~1.5 M output samples clipped, though: the real-IF output at 192 kHz peaks just over
  full scale (the same station without a simulator clips too), which wants a level check.

  *The form offers them* (2026-11-01). The `[simulate]` card's picker now lists the six annex B
  models and the three VHF presets; choosing a VHF one swaps `channel` for `vhf` plus Speed and
  Band rows (0 km/h = parked, 200 MHz = Band III by default), and choosing an HF model removes
  those keys again — `[simulate]` is `deny_unknown_fields`, so a leftover key would either fail
  to load or describe a model that is not in use. `the_vhf_presets_replace_the_hf_channel_number`
  checks the swap, the round trip through the station's parser, and that the speed and band
  survive switching between VHF profiles.

  *DRM+ over MDI, receive side* (2026-11-03). MDI could not express mode E at all: `robm` is
  two bits and TS 102 820 names only A–D, and `fac_` was parsed as a fixed 9 bytes, so a DRM+
  content server's frames were rejected. The FAC is its own marker — §6.3 gives mode E two
  service parameter sets, so its block is 116 bits where A–D's is 72:

  - `decdrm_mdi::mdi::MdiFac` keeps the block's exact width (72 or 116 bits);
    `MdiFrame::is_mode_e()` and `fac_bits()` follow it. A mode E frame carries **no** `robm`.
  - `Session::push_mdi` takes the mode from that width (else `robm`, else the last status) and
    advances time by `mode.frame_ms()` — 100 ms per mode E frame instead of the hardcoded
    400 ms. `Source::position_s` had the same 0.4 s-per-frame assumption for MDI input; it now
    sums the frames' real durations.
  - The station's `capture_mdi` writes `fac.to_bits_for(mode)` (116 bits for DRM+) and omits
    `robm`; its modulator takes the mode from the FAC's width and rejects a mode E stream when
    the station's working rate cannot carry it.

  Measured with `drm_plus_over_mdi` (60 frames of a mode E station at 192 kHz through a `.rsM`
  recording and the engine): mode E in the status, the service from the SDC, **FAC 60/60**,
  SDC 15, and 6.0 s of input — where the old code would have called it 24 s.

  *Mode E audio, end to end* (2026-11-03). Real DRM+ recordings decoded all along: the
  `refs/` recordings are xHE-AAC, and the receiver's deframer finds the 200 ms super frame
  from the xHE-AAC header (the 26 Oct build decodes 459 audio frames of a 20 s reference with
  none concealed; this code state 454, none concealed). What never worked was DecDRM's
  **station** on mode E — it encoded one 400 ms super frame per 100 ms logical frame — and the
  **AAC** branch of the deframer, which had no way to find the phase (the xHE-AAC check never
  passes on an AAC super frame): the old build locks onto this station's mode E AAC signal
  (FAC/SDC fine) and decodes **0** audio frames, where this one decodes 10 of 1.5 s. Now:

  - `AudioPlan` carries `parts_per_super_frame` (1 in A–D, 2 in mode E) and the logical frame
    duration; `AudioChain::next_logical_frame` encodes one super frame (200 ms of PCM in mode E)
    and returns its halves, one per call, holding the second in `pending`.
  - The AAC frame count follows §5.4.1's mode E rates: 5 frames at a 24 kHz core, 10 at 48 kHz
    (where A–D have 12/24 kHz) — `AudioParams::aac_frames_per_super_frame` now takes the mode,
    and Opus gets 10 packets per 200 ms super frame instead of 20.
  - The receiver's `AudioDeframer` takes the phase from the FAC's frame identities when it has
    them (MDI, whose frames carry their own FAC: frames 0 and 2 start a super frame), and
    otherwise finds it from the audio frame borders (AAC/Opus `parse`) or the xHE-AAC header CRC.

  Three more station bugs stood in the way, all found by making a receiver decode the signal:

  - **SDC mode.** Mode E has no 16-QAM SDC; its SDC-mode bit is a protection level (§6.3.3), so
    the station's default `sdc_mode = Qam16` made the encoder send 16-QAM cells while the FAC
    announced 4-QAM. No receiver could decode the SDC (it carries the audio and data
    parameters), which is why the mode E loopback tests never got an audio pipeline at all. A
    16-QAM setting now means the mode's 4-QAM rate 1/2.
  - **MSC mode.** The default `msc_mode = Qam64Sm` cannot be signalled in mode E either: the FAC
    field fell back to 4-QAM while the cells carried 64-QAM. A 64-QAM/hierarchical setting is
    now mapped to 16-QAM for mode E, and 16-QAM's four protection levels (table 31) are accepted
    there where modes A–D have two.
  - **Allocation tables.** `allocate` sized the streams with `MlcParams::msc`, which is
    hardcoded to mode B: mode E streams were computed from A–D's code rates and came out ~5×
    too long (the debug check against the transmitter caught it once the MSC mode agreed). It
    now uses `msc_for(mode, …)`.

  Measured: `drm_plus_audio_decodes_through_the_receiver` (4 s of mode E HE-AAC at 192 kHz,
  I/Q, receiver pinned to mode E) decodes **75 audio frames** with no concealment and the 800 Hz
  tone is the strongest component of the last second; `drm_plus_over_mdi` decodes **150 frames**
  of its 6 s (40 ms AAC frames at the 24 kHz core) with no errors. xHE-AAC and DAC in mode E
  are rejected with a message (their super frames still need the two-frame split).

  *Mode E durations, and simulated stations' headroom* (2026-11-03). Two small fixes from the
  same review:

  - **Bit rates and durations.** The station still divided by 400 ms everywhere: the plan's
    stream bit rates, `MSC … kbit/s`, the data-request sizes and the modulator's service
    rates read a mode E stream 4× low (a 1530-byte frame is 122.4 kbit/s, not 30.6). Mode E
    frames are 100 ms (§8.1), so `StreamPlan` carries its `frame_seconds`, `describe` uses
    `MultiplexPlan::frame_seconds()`, and the `--duration` frame count in the CLI and the GUI
    (`tx_config::frames_for`) take the mode's frame duration — a 60 s run was 15 s long in
    mode E before this.
  - **Output headroom with a simulator.** A `[simulate]` channel adds fading gain on top of
    the signal's own peaks: at the default −15 dBFS a mode E real-IF station clipped ~2000
    samples in 30 s through TU6, where the same station without a simulator clipped none in
    30 s (peaks −2.2 dBFS). `plan.describe` now warns when a simulator leaves less than 6 dB
    of headroom and no limiter is set (it suggests about −20 dBFS or `papr_db`), `Station`
    logs one line when the output first clips, and the example/config docs say so. With a
    6 dB limiter at −15 dBFS, 30 s through TU6 clipped nothing. The VHF loopback test runs at
    −21 dBFS with `papr_db = 6` and asserts no clipped samples: 0 in 30 s measured, where the
    old default clipped 1973.

  *Mode E audio: xHE-AAC, DAC and Opus* (2026-11-03). The two codecs that were rejected in
  mode E now work, and a third one turned out broken:

  - **xHE-AAC**: its super frame in mode E is 200 ms (§5.3.1.0), and the encoder's rate
    control assumed 400 ms everywhere (`bitrate = 20·bytes`, the 15-frame rule, the
    163 920 bit/s ceiling, `frames_per_super_frame`). `XheAacConfig::super_frame_ms` now
    carries the duration and everything scales with it (`with_super_frame_ms`); the station
    plans a 200 ms super frame of two logical frames' bytes and the receiver already found
    the pair from the super frame's header CRC (the DRM+ reference recordings decode that
    way). Measured: 4 s of mode E xHE-AAC at 192 kHz decodes **70 frames**, none concealed.
  - **DAC**: its super frame is 400 ms by construction (30 frames of 320 samples at 24 kHz),
    so mode E cannot hold it in two 100 ms logical frames: it spans **four**. The station
    splits the encoded 400 ms super frame into four parts and the receiver collects four
    (`super_frames_for(Dac, E) = 4`). The FAC frame identity cannot give the phase over RF
    (the interleaver's delay flips the parity), so `AudioDeframer::set_whole_check` lets the
    engine hand the deframer the DAC framing's CRC check: a candidate that fails slides one
    logical frame and tries again. Measured: 4 s of mode E DAC (24 kbit/s, 192 kHz) decodes
    **7 super frames, 0 concealed**, the 440 Hz tone at 0.254 against 0.25.
  - **Opus**: the station built the 400 ms format's twenty borders for mode E's 200 ms
    super frame and so failed to encode it at all (`AacSuperFrameFormat::opus_for` with the
    plan's packet count fixes it). Measured: 150 packets of 4 s, none concealed.

  The transmitter form's audio card no longer greys out xHE-AAC or DAC in mode E; it still
  offers mode E's AAC core rates (24 kHz, and 48 kHz without SBR — §5.4.1) and repairs a rate
  the document holds from the other mode. The receiver's Diversity tab used a hardcoded
  400 ms per record for its time axis; `DiversityView::frame_seconds` now carries the mode's
  (0.1 s in mode E).

  *Portable executables* (2026-11-03). `scripts/build-portable.ps1` gained `-NoWeights` (build
  without the DAC codec for a much smaller executable, or when the weights are not at hand)
  and two wrappers, `scripts/build-portable.bat` (with the weights) and
  `scripts/build-portable-noweights.bat` (without); the script checks for the weights up
  front, so a missing download is one clear line rather than a build-script panic. The build now sets
  `CARGO_PROFILE_RELEASE_DEBUG=0`: with the profile's line tables MSVC embeds the compile
  command line in every object (`/Z7`), and the codec crates' long include list plus this
  workspace's path make `cl` fail with D8050 ("cannot execute c1xx.dll ... debug record").
  The portable binaries do not need debug info.

  Not yet done: the GUI's diversity history still assumes 400 ms frames (receiver-side mode E
  diversity is untested); mode E has no MDI output role (not asked for).

  Next: nothing on the DRM+ list — the acquisition path is the last item (item 4 above), and
  the `params(E)` tuning was done before it. What remains untested is listed on the line
  above.

  *PAPR reduction, measured* (2026-11-03). There was none: the output stage clipped at ±1 and
  that was the whole story, so a mode B signal (10.7 dB crest; 12.8 dB for the real part at the
  IF) either clipped or had to run at about −15 dBFS RMS. `[output] papr_db` (GUI: Output →
  PAPR → *limit peaks*) now enables a limiter on the baseband, **before** the channel filter,
  with two laws — a radial `tanh` (`PeakLaw::Tanh`) and a hard `clip` (`PeakLaw::Clip`, the
  default) — with `σ` the RMS, phase unchanged, and the power lost to the limiting restored by
  the Gaussian model (a Rayleigh amplitude, which a few hundred carriers approximate: measured
  within 0.02 dB), so `level_dbfs` still means the RMS. `examples/papr` and `tests/papr.rs`
  measure it on real frames, mode B 64-QAM and mode E 16-QAM (mode E within 0.2 dB of the
  mode B figures):

  | knee (dB above RMS) | tanh peak | tanh MER | clip peak | clip MER | samples above the knee |
  |---|---|---|---|---|---|
  | none | 10.7 | — | 10.7 | — | — |
  | 12 | 9.2 | 33.7 dB | 10.7 | 143.7 dB | 0.000 % |
  | 9 | 8.1 | 28.4 dB | 9.0 | 54.1 dB | 0.016 % |
  | 7.5 | 7.4 | 25.9 dB | 7.5 | 41.5 dB | 0.27 % |
  | 6 | 6.6 | 23.6 dB | 6.1 | 31.6 dB | 1.7 % |
  | 4.5 | 5.8 | 21.4 dB | 4.7 | 24.7 dB | 5.8 % |

  (The 12 dB clip row touches a couple of samples in 4.8 s, so its MER is meaninglessly large —
  it is there to show that at that knee the clip does essentially nothing.)

  The trade is real and not flattering to `tanh`: at the same 6 dB knee the clip keeps 8 dB more
  MER (11 dB if the comparison is at tanh's slightly higher peak) — a memoryless clip is
  transparent below the ceiling and only touches the top 1.7 % of samples, whereas `tanh` starts
  compressing at half the knee (its gain is already −0.7 dB at `r = γσ/2`) and distorts the
  whole distribution. What the smooth law buys is the *spectrum* of the distortion: its products
  decay much faster than a clip's, which matters when the limiter is the last stage. In this
  chain the transmit filter follows it (the limiter plus filter *is* one iteration of
  clip-and-filter), so that advantage is largely spent — the honest summary is that the limiter
  is a **peak-safety knob**, not a quality improvement, and it is off by default. With a 6 dB
  knee the clip's asymptote is the knee itself (the tanh's is 7.2 dB) and the real part adds
  3 dB, so −10 dBFS (−11 with tanh) keeps the real output clear of ±1; the 128 samples that
  clipped in the 192 kHz VHF loopback above would be covered by that.

  Further iterations of clip-and-filter would push the out-of-band distortion below the filter's
  −70 dB floor, which is already where it is; what ICF would buy here is a *final* clip after the
  filter, which is only useful if the level is set so that the filter's re-growth matters. The
  `±1` clip at the end of the chain already bounds the output; a level at −10 dBFS leaves it
  ~4 dB of margin.

  *The external xHE-AAC encoder* (2026-11-04). libxaac is the weak point of a DRM+ transmission:
  its USAC encoder is the one part of a reference decoder that was never meant to be competitive,
  and at DRM+ bit rates its quality is well below Fraunhofer's. The DLL shipped with the Spark
  Modulator DRM+ transmitter *is* the genuine Fraunhofer encoder plus their DRM+ superframe
  assembler — but it is 32-bit and JNI-only, and DecDRM's executables are 64-bit.

  So it is a side tool, not part of the program. `tools/xhe-host` (a ~330-line C shim) loads the
  DLL in a 32-bit process and speaks a line protocol over stdio — `OK`, `NEED <samples>`,
  `FRAME <bytes>` — calling the DLL's DRM+ entry points directly, so no JVM is involved.
  `scripts\build-xhe-host.bat` builds
  it into `exe\xhe\` beside the portable executables, copying `fhgio.dll` in when it is in
  `refs\dll`. DecDRM looks for `xhe\decdrm-xhe-host.exe` beside itself: without the folder — or
  without the tool in it — the external encoder is simply absent, and nothing else changes.

  With the helper present the station offers `codec = "xhe-aac-external"` (GUI: *xHE-AAC (external
  encoder)*). The DLL returns whole DRM audio super frames (200 ms in DRM+, 400 ms in DRM30) at
  exactly the stream's bit rate, plus the SDC **type-9 body** — two header bytes (coding 3, mode,
  rate code) and the xHE-AAC static config. The plan therefore takes the sampling rate and the SDC
  from the encoder instead of computing them, and the chain has no framer, lead accounting or rate
  control to run. Two services each get their own helper process (the DLL allows one encoder
  instance per process), and the helper's own 640 ms start-up delay pads with silence. The DLL's
  HE-AAC entry never initialised in any probe (its licence profile reports no instances for it),
  so only xHE-AAC is offered; future encoder options will be named `xhe_ext_*`.

  The seam brought two user-interface traps with it. The helper is a console program, so Windows
  opened a console window for every probe; it is now started with `CREATE_NO_WINDOW` (a GUI
  program has no console of its own). And a probe *is* a process start — ~40 ms once the DLL is
  in the OS cache, over a second on the first one while it loads — which the transmitter tab
  used to do on the UI thread, so every edit that changed the multiplex froze the window. Checks
  now run on their own thread and the result is applied when it arrives; the synchronous path
  survives only where a result is needed on the spot (pressing *Transmit*), and it is fast
  because the background check has already cached that configuration.

  It also has to be fed at the rate it *asks*, not at the rate a super frame implies. The encoder
  takes PCM in blocks of `n_smp` floats (1024 stereo frames on this build), and a 200 ms super
  frame is 9.375 of those, so it asks for 9 or 10 blocks per frame — 18432 or 20480 floats against
  the 19200 a super frame holds. Fed from an empty FIFO it starved on the ten-block frames, and the
  silence inserted to keep it going is audible: brief silences and clicks, whose frequency depended
  on the phase of that rotation (so they came and went with unrelated setting changes). The FIFO now
  starts with one super frame of silence (19200 floats stereo in DRM+, 9600 mono, 38400 in DRM30),
  which absorbs the whole rotation; against the helper itself, 3 of 24 super frames starved before
  and none after. It costs that much extra latency, which the DLL's own 640 ms delay makes free.

  The DLL is not uniform across bit rates either, and the reason is one parameter: `nSmp` is
  *in/out*. At the lower bit rates (24–36 kbit/s, where a block is 6144 floats) the encoder often
  takes **none** of the block it is given — it emits the super frame from what it already holds and
  reports `nSmp = 0`, with `left` non-zero — and a host that reads a fresh block anyway has thrown
  that audio away. That cost a fifth of the programme: gaps of 40–55 ms several times a second, with
  the pitch untouched (26 gaps in 8.6 s on a mode E, 4-QAM, protection 2 station with two services at
  29.8 kbit/s). The helper now honours `nSmp`: what the encoder did not take stays in the buffer and
  `NEED` asks only for the rest. That brings the demand back to 19292 floats per 200 ms frame
  (+0.5 %, like every other bit rate) and the same station to **zero gaps**, a flat level and a
  1000.00 Hz tone end to end.

  Measured (mode E, 192 kHz, 16-QAM protection 1): one stereo service at 122.4 kbit/s decodes
  **141 audio frames of 4 s, none concealed**, the 800 Hz tone intact; two services at 61.2 kbit/s
  each decode 70 frames each with none concealed — and the reference DRM+ recordings (from the
  same Spark tool chain) already decode with the built-in path, so the receiver needed nothing
  new.

## Conventions

* Cite the spec in doc comments: `ES 201 980 §7.3.2`. When an algorithm is borrowed from
  Dream, name the Dream file/class it came from.
* No `unsafe` outside the `*-sys` crates and the thin FFI wrappers in `decdrm-codecs`.
* Tests: unit tests per module; integration tests decode files from `samples/` and are
  skipped (not failed) when the samples are absent; loopback tests use our transmitter.
* `samples/` (user-provided recordings) and `reference/` (Dream source, specs) are not
  committed.

## Reference material

* Dream source r1548 (`branches/dream-mjf`) — `reference/dream-mjf/` (fetched over SVN HTTP
  from `https://svn.code.sf.net/p/drm/code/branches/dream-mjf/`).
* ETSI specifications (free; store them in `reference/etsi/`):
  * [ES 201 980 V4.2.1](https://www.etsi.org/deliver/etsi_es/201900_201999/201980/04.02.01_60/es_201980v040201p.pdf) — DRM system specification
    (`reference/etsi/es_201980v040201p.pdf`, plus a `.txt` extraction for grepping)
  * TS 101 968 — DRM data applications directory
  * TS 102 979 — Journaline
  * EN 301 234 — MOT protocol; TS 101 499 — MOT SlideShow; TS 101 498 — Broadcast Website
  * TS 102 818 / TS 102 371 — EPG (XML and binary encoding)
