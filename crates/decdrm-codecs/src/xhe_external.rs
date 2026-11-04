//! The optional **external** xHE-AAC encoder: a small 32-bit helper process
//! (`xhe\decdrm-xhe-host.exe`, see `tools/xhe-host`) that drives Fraunhofer's DRM xHE-AAC
//! encoder DLL. DecDRM's own executables are 64-bit and cannot load that DLL.
//!
//! Everything here is optional by construction: [`available`] looks for the helper beside
//! the running executable, and when the folder or the helper is missing the external
//! encoder is simply not offered. Nothing else in DecDRM depends on it.
//!
//! The helper speaks a line protocol on stdin/stdout:
//!
//! ```text
//! OK <frameBytes> <nSmpFloats> <pcmRate> <channels> <drmPlus> <sdcHex>
//! NEED <n>        -> the host reads n f32 samples (little-endian) from stdin
//! FRAME <len>     -> followed by len bytes of one DRM audio super frame
//! ERROR <code> <text>
//! ```
//!
//! [`ExternalInfo::type9`] is the SDC type-9 body the encoder wants (the audio-service
//! bytes of SDC entity 9, including the xHE-AAC static config), so a station can use it as
//! [`crate::AudioInfo`] and the receiver needs nothing special.
//!
//! The encoder takes 48 kHz interleaved `f32` and holds about 640 ms of audio internally,
//! so the first super frames it returns are silence (that is the encoder's delay, exactly
//! like libxaac's lead). [`ExternalXheEncoder::super_frame`] therefore pads with silence if
//! the caller has not supplied enough PCM yet, and always returns one super frame.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::CodecError;

/// The folder beside the executable that holds the helper and the DLL.
pub const HOST_DIR: &str = "xhe";
/// The helper's file name (`decdrm-xhe-host.exe` on Windows).
pub const HOST_FILE: &str = if cfg!(windows) { "decdrm-xhe-host.exe" } else { "decdrm-xhe-host" };
/// Environment override for the helper's path (tests, and running from `cargo run` where
/// the helper is not beside the build output).
pub const HOST_ENV: &str = "DECDRM_XHE_HOST";

/// What the helper is asked for: the DRM framing, the channel count and the super frame
/// size (the bit rate follows from it: bytes · 8 / T, T = 0.2 s DRM+ or 0.4 s DRM30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExternalConfig {
    pub drm_plus: bool,
    pub channels: u8,
    pub frame_bytes: usize,
}

impl ExternalConfig {
    pub fn new(drm_plus: bool, channels: u8, frame_bytes: usize) -> Self {
        Self { drm_plus, channels, frame_bytes }
    }

    /// The bit rate the helper will ask the encoder for, bit/s (for error messages).
    pub fn bitrate(&self) -> u32 {
        self.frame_bytes as u32 * if self.drm_plus { 40 } else { 20 }
    }
}

/// What the helper reported when it opened the encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInfo {
    /// Size of one DRM audio super frame, bytes.
    pub frame_bytes: usize,
    /// Interleaved `f32` samples the encoder takes per call.
    pub n_smp: usize,
    /// Sampling rate of the PCM fed to the helper (48 kHz).
    pub sample_rate: u32,
    pub channels: u8,
    pub drm_plus: bool,
    /// The SDC type-9 body (2 header bytes plus the xHE-AAC static config), as
    /// [`crate::AudioInfo::from_type9_bytes`] expects it.
    pub type9: Vec<u8>,
}

/// The helper beside the running executable (`xhe/decdrm-xhe-host.exe`), or wherever
/// [`HOST_ENV`] points.
pub fn host_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(HOST_ENV) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(HOST_DIR).join(HOST_FILE);
    p.is_file().then_some(p)
}

/// A default configuration for a capability check (DRM+, stereo, 24 kbit/s).
fn default_config() -> ExternalConfig {
    ExternalConfig::new(true, 2, 600)
}

/// Whether the external encoder is usable: the helper is present and can open the encoder
/// (which needs the DLL beside it). Cached for the life of the process — a helper dropped in
/// place while DecDRM runs is picked up on the next start.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| probe(&default_config()).is_ok())
}

/// One-line description for user interfaces.
pub fn describe() -> String {
    match host_path() {
        Some(p) => format!("external xHE-AAC encoder: {}", p.display()),
        None => "no external xHE-AAC encoder beside the executable".into(),
    }
}

fn failed(what: impl Into<String>) -> CodecError {
    CodecError::External(what.into())
}

fn spawn(cfg: &ExternalConfig, probe: bool) -> Result<(Child, ChildStdin, BufReader<ChildStdout>, ExternalInfo), CodecError> {
    let path = host_path().ok_or_else(|| failed(format!("no external xHE-AAC encoder (looked for {HOST_DIR}\\{HOST_FILE} beside the executable; set {HOST_ENV} to override)")))?;
    let mut cmd = Command::new(&path);
    cmd.arg(if cfg.drm_plus { "--drmplus" } else { "--drm30" })
        .arg("--channels")
        .arg(cfg.channels.to_string())
        .arg("--bytes")
        .arg(cfg.frame_bytes.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if probe {
        cmd.arg("--probe");
    }
    let mut child = cmd.spawn().map_err(|e| failed(format!("cannot start {}: {e}", path.display())))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut out = BufReader::new(stdout);
    let info = read_ok(&mut out, cfg, &path)?;
    Ok((child, stdin, out, info))
}

/// Parse the helper's greeting: `OK <frameBytes> <nSmp> <pcmRate> <channels> <drmPlus> <sdcHex>`.
fn parse_ok(line: &str) -> Option<ExternalInfo> {
    let mut it = line.strip_prefix("OK ")?.split_whitespace();
    let frame_bytes = it.next()?.parse().ok()?;
    let n_smp = it.next()?.parse().ok()?;
    let sample_rate = it.next()?.parse().ok()?;
    let channels = it.next()?.parse().ok()?;
    let drm_plus = match it.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let mut type9 = Vec::new();
    if let Some(hex) = it.next() {
        if hex.len() % 2 != 0 {
            return None;
        }
        for i in (0..hex.len()).step_by(2) {
            type9.push(u8::from_str_radix(&hex[i..i + 2], 16).ok()?);
        }
    }
    Some(ExternalInfo { frame_bytes, n_smp, sample_rate, channels, drm_plus, type9 })
}

fn read_line(out: &mut impl BufRead) -> Result<String, CodecError> {
    let mut line = String::new();
    let n = out.read_line(&mut line).map_err(|e| failed(format!("reading from the encoder host: {e}")))?;
    if n == 0 {
        return Err(failed("the encoder host closed the connection"));
    }
    Ok(line)
}

fn read_ok(out: &mut impl BufRead, cfg: &ExternalConfig, path: &std::path::Path) -> Result<ExternalInfo, CodecError> {
    let line = read_line(out)?;
    if let Some(rest) = line.strip_prefix("ERROR ") {
        return Err(failed(format!("{}: {rest}", path.display())));
    }
    let info = parse_ok(line.trim_end())
        .ok_or_else(|| failed(format!("unexpected greeting from {}: {line:?}", path.display())))?;
    // The helper echoes the request; a mismatch is a bug, not a user error.
    if info.frame_bytes != cfg.frame_bytes || info.channels != cfg.channels || info.drm_plus != cfg.drm_plus {
        return Err(failed(format!(
            "the encoder host opened {} bytes, {} channels, DRM+ {} instead of the requested {} / {} / {}",
            info.frame_bytes, info.channels, info.drm_plus, cfg.frame_bytes, cfg.channels, cfg.drm_plus
        )));
    }
    Ok(info)
}

/// Probe the encoder for `cfg` without keeping it open (cached: the greeting is a pure
/// function of the configuration, and user interfaces ask on every edit).
pub fn probe(cfg: &ExternalConfig) -> Result<ExternalInfo, CodecError> {
    static CACHE: OnceLock<Mutex<HashMap<ExternalConfig, ExternalInfo>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(info) = cache.lock().ok().and_then(|c| c.get(cfg).cloned()) {
        return Ok(info);
    }
    let (mut child, _stdin, mut out, info) = spawn(cfg, true)?;
    // `--probe` exits by itself after the greeting; drain stderr for a useful message.
    let _ = read_line(&mut out);
    let status = child.wait().map_err(|e| failed(format!("waiting for the encoder host: {e}")))?;
    if !status.success() {
        return Err(failed(format!("the encoder host failed (status {status})")));
    }
    if let Ok(mut c) = cache.lock() {
        c.insert(*cfg, info.clone());
    }
    Ok(info)
}

/// A running external encoder.
pub struct ExternalXheEncoder {
    child: Option<Child>,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    info: ExternalInfo,
    /// PCM not yet consumed by the encoder (interleaved).
    fifo: VecDeque<f32>,
    /// A `NEED n` the encoder is waiting for.
    pending: Option<usize>,
    /// Interleaved samples fed as silence because the caller ran short (the start-up
    /// delay, or a caller feeding slower than real time).
    pub padded_samples: u64,
    /// Super frames received.
    pub frames: u64,
}

impl ExternalXheEncoder {
    /// Start the helper and open the encoder for `cfg`.
    pub fn open(cfg: &ExternalConfig) -> Result<Self, CodecError> {
        let (child, stdin, out, info) = spawn(cfg, false)?;
        Ok(Self {
            child: Some(child),
            stdin,
            out,
            info,
            fifo: VecDeque::new(),
            pending: None,
            padded_samples: 0,
            frames: 0,
        })
    }

    /// The greeting: super frame size, the SDC type-9 body, the PCM format.
    pub fn info(&self) -> &ExternalInfo {
        &self.info
    }

    /// Feed one super frame's worth of interleaved PCM (at [`ExternalInfo::sample_rate`])
    /// and return one DRM audio super frame. Pads with silence when the encoder wants more
    /// PCM than the caller has supplied (its start-up delay); never returns early.
    pub fn super_frame(&mut self, pcm: &[f32]) -> Result<Vec<u8>, CodecError> {
        self.fifo.extend(pcm.iter().copied());
        loop {
            if self.pending.is_none() {
                let line = read_line(&mut self.out)?;
                let line = line.trim_end();
                if let Some(rest) = line.strip_prefix("ERROR ") {
                    return Err(failed(format!("the encoder host failed: {rest}")));
                }
                if let Some(n) = line.strip_prefix("NEED ") {
                    let n: usize = n.parse().map_err(|_| failed(format!("bad NEED line: {line:?}")))?;
                    self.pending = Some(n);
                } else if let Some(n) = line.strip_prefix("FRAME ") {
                    let n: usize = n.parse().map_err(|_| failed(format!("bad FRAME line: {line:?}")))?;
                    let mut data = vec![0u8; n];
                    self.out.read_exact(&mut data).map_err(|e| failed(format!("reading a super frame: {e}")))?;
                    self.frames += 1;
                    return Ok(data);
                } else {
                    return Err(failed(format!("unexpected line from the encoder host: {line:?}")));
                }
            }
            let n = self.pending.expect("NEED handling sets this");
            let mut samples = Vec::with_capacity(n);
            for _ in 0..n {
                match self.fifo.pop_front() {
                    Some(v) => samples.push(v),
                    None => {
                        samples.push(0.0);
                        self.padded_samples += 1;
                    }
                }
            }
            self.pending = None;
            let mut bytes = Vec::with_capacity(n * 4);
            for v in &samples {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            self.stdin.write_all(&bytes).map_err(|e| failed(format!("feeding the encoder host: {e}")))?;
            self.stdin.flush().map_err(|e| failed(format!("flushing to the encoder host: {e}")))?;
        }
    }
}

impl Drop for ExternalXheEncoder {
    fn drop(&mut self) {
        // The helper is a separate 32-bit process: it dies with the encoder, and its own
        // exit cleans up the DLL (which crashes if closed twice, so killing is the safe
        // path when the host is still healthy).
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_greeting_parses() {
        let info = parse_ok("OK 600 6144 48000 2 1 D600A22EAA8700").expect("greeting");
        assert_eq!(info.frame_bytes, 600);
        assert_eq!(info.n_smp, 6144);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 2);
        assert!(info.drm_plus);
        assert_eq!(info.type9, [0xD6, 0x00, 0xA2, 0x2E, 0xAA, 0x87, 0x00]);
        assert!(!parse_ok("OK 1200 6144 48000 2 0 D600A22EAA8700").expect("DRM30").drm_plus);
        assert!(parse_ok("garbage").is_none());
        assert!(parse_ok("OK 600 6144 48000 2 1 D60").is_none(), "odd hex");
    }

    #[test]
    fn the_bit_rate_follows_the_frame_size() {
        assert_eq!(ExternalConfig::new(true, 2, 600).bitrate(), 24_000);
        assert_eq!(ExternalConfig::new(true, 2, 2400).bitrate(), 96_000);
        assert_eq!(ExternalConfig::new(false, 2, 1200).bitrate(), 24_000);
    }
}
