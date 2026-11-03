//! MDI and RSCI input end to end: a station's frames as MDI (what a content server
//! sends, `Station::capture_mdi`) go to the engine — from a recording, and over UDP
//! with PFT fragmentation, Reed–Solomon protection, a lost fragment per packet and an
//! RSCI receiver's status items — and the engine decodes the audio, the text and the
//! services without any radio part; RCI commands go back to the "receiver".

use decdrm_engine::decdrm_mdi::file::{DcpFileWriter, FileKind};
use decdrm_engine::decdrm_mdi::net::{UdpOrigin, UdpSender};
use decdrm_engine::decdrm_mdi::pft::{PftConfig, fragment};
use decdrm_engine::decdrm_mdi::rci::{RciCommand, RciListener};
use decdrm_engine::decdrm_mdi::rsci::{ImpulseResponse, RxFlags};
use decdrm_engine::decdrm_mdi::source::MdiOrigin;
use decdrm_engine::decdrm_mdi::{MdiFrame, Protocol, RsciStatus};
use decdrm_engine::{Command, Engine, EngineConfig, EngineEvent, InputSpec, MdiSpec, Snapshot};
use decdrm_station::{Station, StationConfig};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

/// `frames` frames of a mode B, 10 kHz station (AAC tone, a text message) as MDI.
fn station_mdi(frames: u64) -> Vec<MdiFrame> {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"
        [channel]
        mode = "B"
        occupancy = 3
        msc_mode = "64-QAM"
        interleaving = "short"
        [output]
        file = "rf.wav"
        [[service]]
        label = "MDI Test"
        id = 0xD0D0C9
        [service.audio]
        codec = "aac"
        core_rate = 24000
        text = ["Hello MDI"]
        input = { tone_hz = 1000.0 }
    "#;
    let mut cfg = StationConfig::from_toml_str(toml).unwrap();
    cfg.base_dir = Some(dir.path().to_path_buf());
    let mut station = Station::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    station.capture_mdi(true);
    (0..frames)
        .map(|_| {
            station.transmit_frame().unwrap();
            station.last_mdi().cloned().unwrap()
        })
        .collect()
}

/// Collect the engine's log until `done` holds for a snapshot or `limit` passes.
fn run_until(engine: &Engine, log: &mut Vec<String>, limit: Duration, done: impl Fn(&Snapshot) -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(EngineEvent::Log(l)) = engine.recv_event(Duration::from_millis(20)) {
            log.push(l);
        }
        if done(&engine.snapshot()) {
            return true;
        }
    }
    false
}

/// `frames` frames of a mode E (DRM+) station as MDI: 100 ms logical frames whose FACs
/// are 116 bits, the only marker MDI has for DRM+ (`robm` names A–D).
fn station_mdi_drm_plus(frames: u64) -> Vec<MdiFrame> {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"
        [channel]
        mode = "E"
        occupancy = 0
        working_sample_rate = "192"
        [output]
        file = "rf.wav"
        [[service]]
        label = "DRM+ MDI"
        id = 0xD0D0E1
        [service.audio]
        codec = "he-aac"
        input = { tone_hz = 1000.0 }
    "#;
    let mut cfg = StationConfig::from_toml_str(toml).unwrap();
    cfg.base_dir = Some(dir.path().to_path_buf());
    let mut station = Station::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    station.capture_mdi(true);
    (0..frames)
        .map(|_| {
            station.transmit_frame().unwrap();
            station.last_mdi().cloned().unwrap()
        })
        .collect()
}

/// A DRM+ content server's MDI: the engine takes the mode from the 116-bit FAC and the
/// frame duration with it, so a mode E stream is 6 s of signal rather than 24 s.
#[test]
fn drm_plus_over_mdi() {
    let frames = station_mdi_drm_plus(60);
    assert_eq!(frames[0].fac.as_ref().map(|f| f.bits), Some(116), "a mode E FAC is 116 bits");
    assert!(frames[0].is_mode_e());
    assert_eq!(frames[0].robustness, None, "robm cannot name mode E");
    assert!(frames[0].sdc.is_some() && frames[1].sdc.is_none(), "SDC in the first frame of a super frame");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drmplus.rsM");
    let mut w = DcpFileWriter::create(&path, FileKind::FileIo, 0).unwrap();
    for (i, f) in frames.iter().enumerate() {
        w.write_packet(&f.to_af(i as u16).to_bytes(), None).unwrap();
    }
    w.finish().unwrap();

    let origin = MdiOrigin::parse(path.to_str().unwrap()).unwrap();
    let input = InputSpec::Mdi(MdiSpec { origin, realtime: false, rci: None });
    let engine = Engine::start(EngineConfig { input, ..EngineConfig::default() });
    let mut log = Vec::new();
    assert!(run_until(&engine, &mut log, Duration::from_secs(60), |s| s.stopped), "{log:#?}");
    let snap = engine.snapshot();
    drop(engine);
    for l in &log {
        println!("{l}");
    }
    assert_eq!(snap.error, None, "{log:#?}");
    let mdi = snap.input.mdi.as_ref().expect("MDI status");
    assert_eq!((mdi.stats.frames, mdi.stats.lost), (60, 0));
    assert_eq!(snap.rx.mode, Some(decdrm_core::params::RobustnessMode::E), "the FAC's width names the mode");
    assert!((snap.input.position_s - 6.0).abs() < 1e-9, "60 frames of 100 ms, got {} s", snap.input.position_s);
    assert!(snap.services.iter().any(|s| s.label == "DRM+ MDI"), "{:?}", snap.services);
    assert!(snap.rx.fac_ok >= 55 && snap.rx.sdc_ok >= 14, "FAC {} SDC {}", snap.rx.fac_ok, snap.rx.sdc_ok);
    println!("[mdi] mode E: FAC {}/60, SDC {}, {} s", snap.rx.fac_ok, snap.rx.sdc_ok, snap.input.position_s);
    // The multiplex arrives; the audio super frame is the known mode E gap (see
    // `docs/DESIGN.md`: the station still encodes one 400 ms super frame per logical
    // frame), so this only reports what it made of it.
    println!("[mdi] mode E audio: {} frames ok, {} bad", snap.audio.frames_ok, snap.audio.frames_bad);
}

/// An MDI recording (file framing, as Dream writes `.rsA`): decoded to the end.
#[test]
fn mdi_recording_through_the_engine() {
    let frames = station_mdi(25);
    assert_eq!(frames[0].fac.as_ref().map(|f| f.bits), Some(72), "modes A-D have 72-bit FACs");
    assert!(frames[0].sdc.is_some() && frames[1].sdc.is_none(), "SDC in the first frame of a super frame");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.rsM");
    let mut w = DcpFileWriter::create(&path, FileKind::FileIo, 0).unwrap();
    for (i, f) in frames.iter().enumerate() {
        w.write_packet(&f.to_af(i as u16).to_bytes(), None).unwrap();
    }
    w.finish().unwrap();

    let origin = MdiOrigin::parse(path.to_str().unwrap()).unwrap();
    let input = InputSpec::Mdi(MdiSpec { origin, realtime: false, rci: None });
    let engine = Engine::start(EngineConfig { input, ..EngineConfig::default() });
    let mut log = Vec::new();
    assert!(run_until(&engine, &mut log, Duration::from_secs(60), |s| s.stopped), "{log:#?}");
    let snap = engine.snapshot();
    drop(engine);
    for l in &log {
        println!("{l}");
    }
    assert_eq!(snap.error, None, "{log:#?}");
    let mdi = snap.input.mdi.as_ref().expect("MDI status");
    assert_eq!(mdi.protocol.as_deref(), Some("DMDI 0.0"));
    assert_eq!((mdi.stats.frames, mdi.stats.lost), (25, 0));
    assert!(snap.services.iter().any(|s| s.label == "MDI Test"), "{:?}", snap.services);
    assert!(snap.audio.frames_ok >= 150, "{} audio frames ok", snap.audio.frames_ok);
    assert_eq!(snap.audio.frames_bad, 0);
    assert_eq!(snap.text.as_deref(), Some("Hello MDI"));
    assert_eq!(snap.rx.mode, Some(decdrm_core::params::RobustnessMode::B));
    assert!((snap.input.position_s - 10.0).abs() < 1e-9);
    assert!(snap.rx.fac_ok >= 24 && snap.rx.sdc_ok >= 8, "FAC {} SDC {}", snap.rx.fac_ok, snap.rx.sdc_ok);
}

/// What an RSCI receiver adds to each frame.
fn rsci_status() -> RsciStatus {
    RsciStatus {
        profile: Some('A'),
        signal_dbuv: Some(38.5),
        flags: Some(RxFlags::default()),
        wmer_fac_db: Some(21.0),
        wmer_msc_db: Some(18.5),
        mer_db: Some(19.5),
        doppler_hz: Some(0.75),
        delay: vec![(95, 1.5), (99, 2.25)],
        psd_db: Some((0..85).map(|i| if (11..74).contains(&i) { -40.0 } else { -80.0 }).collect()),
        impulse_response: Some(ImpulseResponse { start_ms: -2.0, end_ms: 8.0, db: (0..41).map(|i| if i == 8 { 0.0 } else { -40.0 }).collect() }),
        frequency_hz: Some(6_030_000),
        receiver_info: Some("Test receiver".into()),
        ..RsciStatus::default()
    }
}

/// RSCI over UDP in PFT fragments with Reed–Solomon, one fragment lost per packet: the
/// frames are rebuilt and decoded, the receiver's status shows, and RCI commands go
/// back to it.
#[test]
fn rsci_over_udp_with_losses_and_rci() {
    let mut frames = station_mdi(30);
    for f in &mut frames {
        f.protocol = Some(Protocol::RSCI);
        f.rsci = rsci_status();
    }
    let rci_origin = UdpOrigin { port: 0, group: Some(Ipv4Addr::LOCALHOST), interface: None, source: None };
    let mut rci = RciListener::bind(&rci_origin).unwrap();
    let rci_port = rci.local_addr().unwrap().port();
    let origin = MdiOrigin::Udp(UdpOrigin { port: 0, group: Some(Ipv4Addr::LOCALHOST), interface: None, source: None });
    let input = InputSpec::Mdi(MdiSpec { origin, realtime: false, rci: Some(format!("127.0.0.1:{rci_port}").parse().unwrap()) });
    let engine = Engine::start(EngineConfig { input, ..EngineConfig::default() });
    let mut log = Vec::new();
    let listening = |s: &Snapshot| s.input.mdi.as_ref().is_some_and(|m| m.local.is_some());
    assert!(run_until(&engine, &mut log, Duration::from_secs(10), listening), "{log:#?}");
    let local = engine.snapshot().input.mdi.unwrap().local.unwrap();
    let port: u16 = local.rsplit(':').next().unwrap().parse().unwrap();

    // The "receiver": every frame in fragments with FEC; the second fragment is lost.
    let sender = std::thread::spawn(move || {
        let tx = UdpSender::new(&format!("127.0.0.1:{port}").parse().unwrap()).unwrap();
        let cfg = PftConfig { max_payload: 600, fec: Some(2), ..PftConfig::default() };
        for (n, f) in frames.iter().enumerate() {
            for (i, frag) in fragment(&f.to_af(n as u16).to_bytes(), n as u16, &cfg).iter().enumerate() {
                if i != 1 {
                    tx.send(&frag.to_bytes()).unwrap();
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
    let decoded = |s: &Snapshot| s.audio.frames_ok >= 100 && s.text.is_some();
    assert!(run_until(&engine, &mut log, Duration::from_secs(60), decoded), "{log:#?}");
    sender.join().unwrap();

    let snap = engine.snapshot();
    let mdi = snap.input.mdi.clone().unwrap();
    assert_eq!(mdi.protocol.as_deref(), Some("RSCI 3.0"));
    assert!(mdi.stats.pft.recovered >= 20, "{:?}", mdi.stats);
    assert_eq!(mdi.rsci.profile, Some('A'));
    assert_eq!(mdi.rsci.receiver_info.as_deref(), Some("Test receiver"));
    assert_eq!((snap.rx.wmer_db, snap.rx.mer_db, snap.rx.fac_mer_db), (Some(18.5), Some(19.5), Some(21.0)));
    assert_eq!((snap.rx.doppler_hz, snap.rx.delay_ms), (0.75, 1.5));
    assert_eq!(snap.rx.state, decdrm_core::rx::RxState::Locked);
    assert_eq!(snap.visuals.spectrum_db.len(), 85);
    assert_eq!(snap.visuals.dc_hz, Some(0.0));
    assert!((snap.visuals.spectrum_centre_hz - (-7875.0 + 85.0 * 187.5 / 2.0)).abs() < 1e-9);
    assert_eq!(snap.visuals.chain.pds.len(), 41);
    assert!((snap.visuals.chain.pds_axis.unwrap().step_ms - 0.25).abs() < 1e-12);
    assert_eq!(snap.text.as_deref(), Some("Hello MDI"));

    // RCI: retune and select a service.
    engine.command(Command::Tune(7325.0));
    engine.command(Command::SelectService(0));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.len() < 2 && Instant::now() < deadline {
        got.extend(rci.poll(Duration::from_millis(100)).unwrap().into_iter().map(|(c, _)| c));
    }
    drop(engine);
    assert_eq!(got, [RciCommand::Frequency(7_325_000), RciCommand::Service(0)]);
}

/// Decode a recording with the receiver session: (audio frames ok, texts, labels).
fn decode(path: &std::path::Path) -> (u64, u64, Vec<String>, Vec<String>) {
    use decdrm_engine::{InputFormat, RealChannel, ReceiverConfig, Session, SessionEvent};
    let mut reader = decdrm_io::FileReader::open(path).unwrap();
    let channels = reader.format().channels;
    let mut session = Session::new(ReceiverConfig { input: InputFormat::Real(RealChannel::Mix), channels, ..Default::default() });
    let mut texts = Vec::new();
    while let Some(block) = reader.read(4800).unwrap() {
        for ev in session.push(&block) {
            if let SessionEvent::Text(Some(t)) = ev {
                texts.push(t);
            }
        }
    }
    let labels = session.ensemble().services().filter_map(|s| s.label.clone()).collect();
    (session.audio_stats.frames_ok, session.audio_stats.frames_concealed, texts, labels)
}

/// The modulator: a station with `[mdi]` transmits an MDI recording; the receiver
/// decodes the signal as if the content server's own station had sent it.
#[test]
fn modulator_transmits_an_mdi_recording() {
    let frames = station_mdi(30);
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("studio.rsM");
    let mut w = DcpFileWriter::create(&rec, FileKind::FileIo, 0).unwrap();
    // Start mid-super frame: the modulator waits for the first frame of one.
    for (i, f) in frames.iter().enumerate().skip(1) {
        w.write_packet(&f.to_af(i as u16).to_bytes(), None).unwrap();
    }
    w.finish().unwrap();
    let toml = r#"
        [mdi]
        input = "studio.rsM"
        [output]
        file = "modulated.wav"
    "#;
    let mut cfg = StationConfig::from_toml_str(toml).unwrap();
    cfg.base_dir = Some(dir.path().to_path_buf());
    let plan = cfg.validate().unwrap_or_else(|e| panic!("{e}"));
    assert!(plan.describe(&cfg).starts_with("modulator: MDI from studio.rsM"), "{}", plan.describe(&cfg));
    assert!(cfg.inputs_finite());
    let mut station = Station::with_plan(cfg, plan).unwrap();
    let mut log = Vec::new();
    let t0 = Instant::now();
    while !station.inputs_finished() && t0.elapsed() < Duration::from_secs(60) {
        station.transmit_frame().unwrap();
        log.extend(station.take_log());
    }
    let status = station.finish().unwrap();
    for l in &log {
        println!("{l}");
    }
    let m = status.mdi.clone().expect("modulator status");
    // Frames 1 and 2 belong to a super frame begun before the recording: skipped.
    assert_eq!((m.frames, m.fillers), (27, 0), "{m:?}");
    assert_eq!(m.channel.as_deref(), Some("mode B, 10 kHz, 64-QAM, short interleaving"));
    assert_eq!(status.frames, 27);
    assert!(status.services.iter().any(|s| s.label == "MDI Test" && s.audio.is_some()), "{:?}", status.services);

    let (ok, concealed, texts, labels) = decode(&dir.path().join("modulated.wav"));
    assert!(ok >= 100, "{ok} audio frames ok");
    assert_eq!(concealed, 0);
    assert!(texts.iter().any(|t| t == "Hello MDI"), "{texts:?}");
    assert_eq!(labels, ["MDI Test"]);
}

/// Live MDI over UDP with a frame lost: the modulator sends a filler in its place and
/// carries on.
#[test]
fn modulator_fills_lost_frames() {
    let frames = station_mdi(30);
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let toml = format!(
        r#"
        [mdi]
        input = "127.0.0.1:{port}"
        [output]
        file = "live.wav"
    "#
    );
    let mut cfg = StationConfig::from_toml_str(&toml).unwrap();
    cfg.base_dir = Some(dir.path().to_path_buf());
    assert!(!cfg.inputs_finite());
    let mut station = Station::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    let sender = std::thread::spawn(move || {
        let tx = UdpSender::new(&format!("127.0.0.1:{port}").parse().unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        for (n, f) in frames.iter().enumerate() {
            if n != 10 {
                tx.send(&f.to_af(n as u16).to_bytes()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let t0 = Instant::now();
    while station.status().frames < 30 && t0.elapsed() < Duration::from_secs(60) {
        station.transmit_frame().unwrap();
    }
    sender.join().unwrap();
    let log = station.take_log();
    let status = station.finish().unwrap();
    let m = status.mdi.clone().unwrap();
    assert_eq!((m.frames, m.fillers), (29, 1), "{m:?} {log:#?}");
    assert_eq!(m.link.lost, 1);
    let (ok, _, texts, _) = decode(&dir.path().join("live.wav"));
    assert!(ok >= 100, "{ok} audio frames ok");
    assert!(texts.iter().any(|t| t == "Hello MDI"), "{texts:?}");
}

/// Remote control: RCI commands sent to the engine select a service and ask for a
/// retune (which an MDI recording cannot follow: the log says so).
#[test]
fn remote_control_by_rci() {
    use decdrm_engine::decdrm_mdi::rci::RciSender;
    let frames = station_mdi(25);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.rsM");
    let mut w = DcpFileWriter::create(&path, FileKind::FileIo, 0).unwrap();
    for (i, f) in frames.iter().enumerate() {
        w.write_packet(&f.to_af(i as u16).to_bytes(), None).unwrap();
    }
    w.finish().unwrap();
    let origin = MdiOrigin::parse(path.to_str().unwrap()).unwrap();
    let input = InputSpec::Mdi(MdiSpec { origin, realtime: true, rci: None });
    let rci_listen = Some(UdpOrigin { port: 0, group: Some(Ipv4Addr::LOCALHOST), interface: None, source: None });
    let engine = Engine::start(EngineConfig { input, rci_listen, ..EngineConfig::default() });
    let mut log = Vec::new();
    assert!(run_until(&engine, &mut log, Duration::from_secs(10), |s| s.remote.is_some()), "{log:#?}");
    let listen = engine.snapshot().remote.unwrap().listen;
    let port: u16 = listen.rsplit(':').next().unwrap().parse().unwrap();
    let mut rci = RciSender::new(&format!("127.0.0.1:{port}").parse().unwrap()).unwrap();
    rci.send(&[RciCommand::Service(0), RciCommand::Frequency(9_710_000), RciCommand::Recording { kind: *b"iq", profile: b'A', on: true }]).unwrap();
    assert!(run_until(&engine, &mut log, Duration::from_secs(10), |s| s.remote.as_ref().is_some_and(|r| r.commands >= 2)), "{log:#?}");
    let remote = engine.snapshot().remote.unwrap();
    drop(engine);
    assert_eq!(remote.last.as_deref(), Some("tune to 9710.000 kHz"));
    let has = |text: &str| log.iter().any(|l| l.contains(text));
    assert!(has("select service 0"), "{log:#?}");
    assert!(has("can be retuned"), "{log:#?}");
    assert!(has("start iq recording (profile A): not supported"), "{log:#?}");
}
