//! Source bar: recording, sound card, KiwiSDR or MDI/RSCI, input format, spectrum
//! options, audio output and the Start / Stop / Restart buttons; the ⚙ menu holds the
//! remote control (RCI).

use crate::settings::{ChannelChoice, ReceiverMode, Settings, SignalFormat, SourceKind};
use decdrm_core::params::{RobustnessMode, WorkingRate};
use eframe::egui::{self, ComboBox, RichText, Ui};
use std::path::Path;

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAction {
    Start,
    Stop,
    Restart,
    /// Open the list of public KiwiSDRs.
    FindKiwi,
    /// Retune the running KiwiSDR (or RSCI receiver) to the frequency in the bar.
    Tune,
}

/// File name extensions of MDI/RSCI recordings for the Open dialog (Dream writes
/// `.rsA`…`.rsZ`, the letter being the RSCI profile).
pub const MDI_EXTENSIONS: &[&str] =
    &["rsa", "rsb", "rsc", "rsd", "rsq", "rsm", "ff", "af", "pf", "pft", "mdi", "dcp", "rsci", "pcap", "pcapng"];

/// Sound-card names, enumerated on first use (WASAPI/ALSA enumeration takes a moment,
/// so it is not done every frame) and on "refresh".
#[derive(Debug, Default)]
pub struct DeviceLists {
    inputs: Option<Vec<NamedDevice>>,
    outputs: Option<Vec<NamedDevice>>,
    pub error: Option<String>,
}

/// A device name and the rates it can carry, as the driver reports them. The name alone
/// does not identify a device when a driver exposes the same virtual cable twice at
/// different rates (VB-Audio does: "CABLE-A Output" appears at both 48 and 192 kHz), and
/// picking the low-rate twin silently band-limits the input below what mode E needs.
#[derive(Debug, Clone)]
pub struct NamedDevice {
    /// The name to store in the settings and to open the device with.
    pub name: String,
    /// Distinct supported rates, ascending; empty when the driver reports none.
    pub rates: Vec<u32>,
}

impl NamedDevice {
    /// The label shown in a picker: the name plus its rates, so endpoints that share a
    /// name are told apart. Falls back to the bare name when no rate is known.
    pub fn label(&self) -> String {
        const MOST: usize = 4;
        if self.rates.is_empty() {
            return self.name.clone();
        }
        let shown = self.rates.len().min(MOST);
        let mut text = String::with_capacity(self.name.len() + 24);
        text.push_str(&self.name);
        text.push_str(" — ");
        for (i, r) in self.rates[..shown].iter().enumerate() {
            if i > 0 {
                text.push_str(", ");
            }
            text.push_str(&khz(*r));
        }
        if shown < self.rates.len() {
            text.push('…');
        }
        text
    }
}

/// A rate in kHz without losing the fractional part: 44100 Hz is "44.1 kHz", not "44 kHz"
/// (integer division would misreport the one rate that is commonly fractional).
fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", f64::from(hz) / 1000.0)
    }
}

impl DeviceLists {
    pub fn inputs(&mut self) -> &[NamedDevice] {
        if self.inputs.is_none() {
            self.inputs = Some(named(decdrm_io::list_input_devices(), &mut self.error));
        }
        self.inputs.as_deref().unwrap_or_default()
    }

    pub fn outputs(&mut self) -> &[NamedDevice] {
        if self.outputs.is_none() {
            self.outputs = Some(named(decdrm_io::list_output_devices(), &mut self.error));
        }
        self.outputs.as_deref().unwrap_or_default()
    }

    pub fn refresh(&mut self) {
        *self = Self::default();
    }
}

fn named(
    list: decdrm_io::Result<Vec<decdrm_io::DeviceInfo>>,
    error: &mut Option<String>,
) -> Vec<NamedDevice> {
    match list {
        Ok(devices) => devices
            .into_iter()
            .map(|d| {
                let rates = d.sample_rates();
                NamedDevice {
                    name: d.name,
                    rates,
                }
            })
            .collect(),
        Err(e) => {
            *error = Some(format!("sound-card enumeration failed: {e}"));
            Vec::new()
        }
    }
}

/// Run `add` greyed out and inert unless `on`. Unlike `add_enabled_ui`, which puts the
/// controls in a child area that wraps on its own, they stay in the caller's row.
/// (Rust note: a closure `FnOnce(&mut Ui) -> Response` is itself a widget, so
/// `add_enabled` can run it; `out` carries its result out.)
fn enabled<R>(ui: &mut Ui, on: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut out = None;
    ui.add_enabled(on, |ui: &mut Ui| {
        out = Some(add(ui));
        ui.response()
    });
    out.expect("add_enabled runs the closure")
}

/// Draw the bar. Controls that define the source are locked while the engine runs
/// (they take effect on the next Start), except the frequency of a running KiwiSDR
/// (`tunable`), which retunes it.
pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    devices: &mut DeviceLists,
    running: bool,
    stopping: bool,
    tunable: bool,
) -> Option<SourceAction> {
    let mut action = None;
    let free = !running;
    ui.horizontal_wrapped(|ui| {
        enabled(ui, free, |ui| {
            ui.selectable_value(&mut settings.source, SourceKind::File, "Recording");
            ui.selectable_value(&mut settings.source, SourceKind::Device, "Sound card");
            ui.selectable_value(&mut settings.source, SourceKind::Kiwi, "KiwiSDR")
                .on_hover_text("Receive from a KiwiSDR on the internet: DecDRM tunes it and takes its I/Q.");
            ui.selectable_value(&mut settings.source, SourceKind::Mdi, "MDI/RSCI").on_hover_text(
                "A DRM multiplex decoded elsewhere, over UDP: MDI from a content server, or RSCI from a receiver \
                 (Dream, a monitoring receiver) with its status. No radio part: the services are decoded straight away.",
            );
            ui.separator();
        });
        match settings.source {
            SourceKind::File => enabled(ui, free, |ui| file_picker(ui, settings)),
            SourceKind::Device => enabled(ui, free, |ui| device_picker(ui, settings, devices)),
            SourceKind::Kiwi => action = kiwi_picker(ui, settings, free, tunable),
            SourceKind::Mdi => action = mdi_picker(ui, settings, free, tunable),
        }
        let mdi_file = settings.source == SourceKind::File
            && settings.file.as_deref().is_some_and(decdrm_engine::decdrm_mdi::file::has_recording_extension);
        enabled(ui, free, |ui| {
            ui.separator();
            if settings.source == SourceKind::Kiwi {
                ui.label(RichText::new("I/Q").weak()).on_hover_text("A KiwiSDR delivers I/Q; the format setting does not apply.");
            } else if settings.source == SourceKind::Mdi || mdi_file {
                ui.label(RichText::new("multiplex").weak())
                    .on_hover_text("MDI/RSCI carries the decoded multiplex: no signal format, no spectrum options.");
            } else {
                format_picker(ui, settings);
            }
            if settings.source != SourceKind::Mdi && !mdi_file {
                ui.checkbox(&mut settings.flip, "Flip").on_hover_text("Mirror the spectrum (e.g. LSB reception).");
                ui.checkbox(&mut settings.auto_flip, "Auto-flip")
                    .on_hover_text("Also accept spectrally inverted signals during acquisition.");
                rate_picker(ui, settings);
                mode_picker(ui, settings);
            }
            let mdi_recording = settings.source == SourceKind::Mdi
                && decdrm_engine::decdrm_mdi::file::has_recording_extension(Path::new(settings.mdi.origin.trim()));
            if settings.source == SourceKind::File || mdi_recording {
                ui.checkbox(&mut settings.realtime, "Real time")
                    .on_hover_text("Pace the recording to real time instead of decoding it as fast as possible.");
            }
            ui.separator();
            ui.checkbox(&mut settings.play_audio, "Audio").on_hover_text("Play the decoded audio.");
            if settings.play_audio {
                output_picker(ui, settings, devices);
            }
        });
        ui.separator();
        if running {
            let stop = ui.add_enabled(!stopping, egui::Button::new("■ Stop"));
            if stop.clicked() {
                action = Some(SourceAction::Stop);
            }
            if ui.button("↺ Restart").on_hover_text("Restart signal acquisition.").clicked() {
                action = Some(SourceAction::Restart);
            }
        } else if ui.button(RichText::new("▶ Start").strong()).clicked() {
            action = Some(SourceAction::Start);
        }
        enabled(ui, free, |ui| receiver_options(ui, settings));
    });
    if let Some(e) = &devices.error {
        ui.colored_label(ui.visuals().warn_fg_color, e);
    }
    action
}

fn file_picker(ui: &mut Ui, settings: &mut Settings) {
    let name = settings
        .file
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(
            || "no recording selected".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
    let label = ui.add(egui::Label::new(RichText::new(name).monospace()).truncate());
    if let Some(path) = &settings.file {
        label.on_hover_text(path.display().to_string());
    }
    if ui.button("Open…").clicked() {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open a DRM recording")
            .add_filter("Recordings (WAV, FLAC)", &["flac", "wav"])
            .add_filter("Multiplex recordings (MDI/RSCI, pcap)", MDI_EXTENSIONS)
            .add_filter("All files", &["*"]);
        if let Some(dir) = settings
            .file
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| d.is_dir())
        {
            dialog = dialog.set_directory(dir);
        }
        // Rust note: `pick_file` blocks this (UI) thread while the native dialog is
        // open; the engine keeps running on its own thread meanwhile.
        if let Some(path) = dialog.pick_file() {
            settings.open_file(path);
        }
    }
}

/// Address (and those used before), frequency, name and password. While a KiwiSDR
/// runs (`tunable`) only the frequency can be changed, and a new one retunes it once
/// typed (Enter) or dragged to. `free`: nothing runs. Returns "Find…" or a retune.
fn kiwi_picker(ui: &mut Ui, settings: &mut Settings, free: bool, tunable: bool) -> Option<SourceAction> {
    use decdrm_engine::decdrm_kiwi::frequency_from_url;
    let mut action = None;
    let k = &mut settings.kiwi;
    enabled(ui, free, |ui| {
        let edit = ui
            .add(egui::TextEdit::singleline(&mut k.address).desired_width(200.0).hint_text("KiwiSDR address"))
            .on_hover_text("host, host:port (port 8073 if left out), or a URL copied from the browser, whose f= also sets the frequency");
        if edit.changed()
            && let Some(f) = frequency_from_url(&k.address)
        {
            k.freq_khz = f;
        }
        if !k.recent.is_empty() {
            // An empty combo box: just its arrow, opening the list.
            ComboBox::from_id_salt("kiwi_recent")
                .width(16.0)
                .selected_text("")
                .show_ui(ui, |ui| {
                    for r in k.recent.clone() {
                        if ui.selectable_label(r.eq_ignore_ascii_case(k.address.trim()), &r).clicked() {
                            k.address = r;
                        }
                    }
                })
                .response
                .on_hover_text("KiwiSDRs used before");
        }
        ui.add(egui::TextEdit::singleline(&mut k.address2).desired_width(150.0).hint_text("2nd KiwiSDR (diversity)"))
            .on_hover_text(
                "Diversity reception: a second KiwiSDR far from the first, on the same frequency. Their signals fade \
                 independently, and the two are combined before decoding: fewer dropouts. Leave empty for one KiwiSDR.",
            );
    });
    // A retune waits for the end of an edit: typing sets the value on Enter (or when the
    // box loses focus), dragging when the mouse is released; the arrow keys step it.
    let freq = ui
        .add_enabled(
            free || tunable,
            egui::DragValue::new(&mut k.freq_khz)
                .range(0.0..=32_000.0)
                .speed(1.0)
                .max_decimals(1)
                .suffix(" kHz")
                .update_while_editing(!tunable),
        )
        .on_hover_text(if tunable {
            "Frequency the KiwiSDR is tuned to: type a new one and press Enter, or drag, to retune it"
        } else {
            "Frequency to tune the KiwiSDR to: the DRM frequency"
        });
    if tunable && ((freq.changed() && !freq.dragged()) || freq.drag_stopped()) {
        action = Some(SourceAction::Tune);
    }
    enabled(ui, free, |ui| kiwi_options(ui, settings, &mut action));
    action
}

/// The ⚙ menu (name, password) and "Find…".
fn kiwi_options(ui: &mut Ui, settings: &mut Settings, action: &mut Option<SourceAction>) {
    let k = &mut settings.kiwi;
    ui.menu_button("⚙", |ui| {
        egui::Grid::new("kiwi_options").num_columns(2).show(ui, |ui| {
            ui.label("Your name");
            ui.add(egui::TextEdit::singleline(&mut k.name).desired_width(160.0).char_limit(32));
            ui.end_row();
            ui.label("");
            ui.label(RichText::new("shown in the KiwiSDR's list of users").weak().small());
            ui.end_row();
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut k.password).password(true).desired_width(160.0));
            ui.end_row();
            ui.label("");
            ui.label(RichText::new("only for KiwiSDRs that need one; not saved").weak().small());
            ui.end_row();
        });
    })
    .response
    .on_hover_text("Your name on the KiwiSDR, and a password");
    if ui.button("Find…").on_hover_text("Choose from the public KiwiSDRs whose owners allow apps").clicked() {
        *action = Some(SourceAction::FindKiwi);
    }
}

/// The MDI/RSCI source: where it comes from (or a recording), where RCI commands go,
/// and the frequency to tune an RSCI receiver to (typed while it runs: retune).
fn mdi_picker(ui: &mut Ui, settings: &mut Settings, free: bool, tunable: bool) -> Option<SourceAction> {
    let mut action = None;
    let m = &mut settings.mdi;
    enabled(ui, free, |ui| {
        ui.add(egui::TextEdit::singleline(&mut m.origin).desired_width(170.0).hint_text("UDP port or group:port"))
            .on_hover_text(
                "Where the MDI/RSCI comes to: a UDP port (8000), a multicast group (239.1.2.3:8000), an interface \
                 and group (192.168.1.5:239.1.2.3:8000), a sender too (10.0.0.9:192.168.1.5:239.1.2.3:8000) — or a \
                 recording (.rsA, .pcap, …)",
            );
        if ui.small_button("…").on_hover_text("Choose an MDI/RSCI recording").clicked() {
            let mut dialog = rfd::FileDialog::new()
                .set_title("Open an MDI/RSCI recording")
                .add_filter("Multiplex recordings (MDI/RSCI, pcap)", MDI_EXTENSIONS)
                .add_filter("All files", &["*"]);
            if let Some(dir) = Path::new(m.origin.trim()).parent().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            if let Some(path) = dialog.pick_file() {
                m.origin = path.display().to_string();
            }
        }
        ui.add(egui::TextEdit::singleline(&mut m.rci).desired_width(130.0).hint_text("RCI to (optional)")).on_hover_text(
            "Remote control of the RSCI receiver: its RCI address (port, or host:port). The frequency below \
             retunes it, and choosing a service selects it there too.",
        );
    });
    let can_tune = !m.rci.trim().is_empty();
    let freq = ui
        .add_enabled(
            (free && can_tune) || tunable,
            egui::DragValue::new(&mut m.freq_khz)
                .range(0.0..=32_000.0)
                .speed(1.0)
                .max_decimals(1)
                .suffix(" kHz")
                .update_while_editing(!tunable),
        )
        .on_hover_text("Frequency to tune the RSCI receiver to (by RCI): type and press Enter while it runs")
        .on_disabled_hover_text("Tuning needs the RSCI receiver's RCI address");
    if tunable && ((freq.changed() && !freq.dragged()) || freq.drag_stopped()) {
        action = Some(SourceAction::Tune);
    }
    action
}

/// The ⚙ menu: the remote control (RCI) of this receiver.
fn receiver_options(ui: &mut Ui, settings: &mut Settings) {
    let on = !settings.rci_listen.trim().is_empty();
    ui.menu_button(if on { "⚙ RCI" } else { "⚙" }, |ui| {
        ui.label(RichText::new("Remote control (RCI)").strong());
        ui.horizontal(|ui| {
            ui.label("Listen on");
            ui.add(egui::TextEdit::singleline(&mut settings.rci_listen).desired_width(120.0).hint_text("UDP port"));
        });
        ui.label(
            RichText::new(
                "RCI commands (TS 102 349, as Dream sends them) to this port tune a KiwiSDR or an RSCI receiver \
                 and select services. Empty: off. Takes effect at the next Start.",
            )
            .weak()
            .small(),
        );
    })
    .response
    .on_hover_text(if on { "Remote control (RCI) is on" } else { "Receiver options: remote control (RCI)" });
}

fn device_picker(ui: &mut Ui, settings: &mut Settings, devices: &mut DeviceLists) {
    // The closed box shows the rate too: two endpoints of one virtual cable share a name
    // and differ only in the rates they accept (VB-Audio lists "CABLE-A Output" at both
    // 48 and 192 kHz), so the bare name cannot tell the user which one is selected.
    let current = match &settings.input_device {
        None => "System default".to_string(),
        Some(want) => devices
            .inputs()
            .iter()
            .find(|d| &d.name == want)
            .map(NamedDevice::label)
            .unwrap_or_else(|| want.clone()),
    };
    ComboBox::from_id_salt("input_device")
        .width(260.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut settings.input_device, None, "System default");
            for d in devices.inputs().to_vec() {
                ui.selectable_value(&mut settings.input_device, Some(d.name.clone()), d.label());
            }
        });
    if ui
        .small_button("⟳")
        .on_hover_text("Refresh the device list")
        .clicked()
    {
        devices.refresh();
    }
}

fn output_picker(ui: &mut Ui, settings: &mut Settings, devices: &mut DeviceLists) {
    // Like the input picker, show the endpoint's rates: a virtual cable lists the same name
    // at 48 and 192 kHz and only one of them can carry a mode E signal.
    let current = match &settings.output_device {
        None => "Default output".to_string(),
        Some(want) => devices
            .outputs()
            .iter()
            .find(|d| &d.name == want)
            .map(NamedDevice::label)
            .unwrap_or_else(|| want.clone()),
    };
    ComboBox::from_id_salt("output_device")
        .width(180.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut settings.output_device, None, "Default output");
            for d in devices.outputs().to_vec() {
                ui.selectable_value(&mut settings.output_device, Some(d.name.clone()), d.label());
            }
        });
}

/// The receiver's working sample rate. It is what the OFDM demodulation runs at, and the
/// input is resampled to it; modes A–D live at 48 kHz, mode E (DRM+) needs 120 kHz or more.
fn rate_picker(ui: &mut Ui, settings: &mut Settings) {
    ComboBox::from_id_salt("working_rate")
        .selected_text(format!("{} kHz", settings.working_rate_hz / 1000))
        .show_ui(ui, |ui| {
            // `WorkingRate`'s own Display writes "48 kHz", "120 kHz", …
            for r in WorkingRate::ALL {
                ui.selectable_value(&mut settings.working_rate_hz, r.hz(), r.to_string());
            }
        })
        .response
        .on_hover_text(
            "Working sample rate of the receiver: what the OFDM chain demodulates at, and what the input is \
             resampled to. Modes A–D are received at 48 kHz; robustness mode E (DRM+) carries 213 carriers \
             reaching ±47.11 kHz and needs 120 kHz or more (96 kHz leaves only 0.89 kHz of Nyquist margin). \
             A rate above 48 kHz does not by itself find DRM+: the automatic search cannot detect mode E, so \
             a DRM+ signal also needs the mode chosen beside this.",
        );
}

/// The robustness mode to decode: automatic search for DRM30, or mode E (DRM+) pinned.
///
/// Only mode E needs pinning. The automatic search covers modes A–D and reads their
/// spectrum occupancy from the FAC; it cannot find mode E, whose 2-bit FAC field cannot
/// express the mode and whose geometry is outside the scan's 48 kHz design. Pinning also
/// tells the receiver the signal is at DC — it does not search for a frequency — so the
/// input must already be tuned to the DRM+ carrier centre.
fn mode_picker(ui: &mut Ui, settings: &mut Settings) {
    ComboBox::from_id_salt("receiver_mode")
        .selected_text(settings.receiver_mode.label())
        .show_ui(ui, |ui| {
            // The choice is taken here and applied once, so the rate can follow it.
            let mut choice = settings.receiver_mode;
            for m in ReceiverMode::ALL {
                ui.selectable_value(&mut choice, m, m.label());
            }
            if choice != settings.receiver_mode {
                settings.receiver_mode = choice;
                // Mode E's carriers reach ±47.11 kHz, so 48 kHz cannot hold them: choosing
                // DRM+ raises the rate to the lowest that fits (120 kHz) rather than leaving
                // a receiver that could only decode noise. 192 kHz is also offered, with
                // more margin around the 98 kHz signal.
                if !settings.working_rate().allows(RobustnessMode::E) {
                    settings.working_rate_hz = WorkingRate::R120.hz();
                }
            }
        })
        .response
        .on_hover_text(
            "Robustness mode. Auto searches for a DRM30 signal (modes A–D) and reads its spectrum \
             occupancy from the FAC. Mode E (DRM+) cannot be found by that search and must be chosen \
             here; its signal has to sit at the receiver's centre frequency (tune to the DRM+ carrier \
             centre) and needs a working rate of 120 or 192 kHz.",
        );
}

fn format_picker(ui: &mut Ui, settings: &mut Settings) {
    ComboBox::from_id_salt("signal_format")
        .selected_text(settings.format.label())
        .show_ui(ui, |ui| {
            for f in SignalFormat::ALL {
                ui.selectable_value(&mut settings.format, f, f.label());
            }
        })
        .response
        .on_hover_text(
            "How the samples represent the signal: a real IF / audio signal, or complex I/Q.",
        );
    if settings.format == SignalFormat::Real {
        ComboBox::from_id_salt("real_channel")
            .selected_text(settings.real_channel.label())
            .show_ui(ui, |ui| {
                for c in ChannelChoice::ALL {
                    ui.selectable_value(&mut settings.real_channel, c, c.label());
                }
            })
            .response
            .on_hover_text("Channel carrying the signal (stereo sources).");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_tells_endpoints_apart_by_their_rates() {
        // VB-Audio lists one virtual cable twice under the same name; only the rate
        // distinguishes a usable mode E input from one that would be band-limited.
        let low = NamedDevice { name: "CABLE-A Output (VB-Audio Cable A)".into(), rates: vec![48_000] };
        let high = NamedDevice { name: low.name.clone(), rates: vec![48_000, 192_000] };
        assert_ne!(low.label(), high.label());
        assert!(low.label().ends_with("48 kHz"));
        assert!(high.label().ends_with("48 kHz, 192 kHz"));
    }

    #[test]
    fn the_label_is_the_bare_name_when_no_rate_is_known() {
        let d = NamedDevice { name: "Some Device".into(), rates: Vec::new() };
        assert_eq!(d.label(), "Some Device");
    }

    #[test]
    fn a_long_rate_list_is_cut_short() {
        let d = NamedDevice {
            name: "Everything".into(),
            rates: vec![8_000, 16_000, 32_000, 44_100, 48_000, 96_000, 192_000],
        };
        let label = d.label();
        assert!(label.starts_with("Everything — 8 kHz"), "{label}");
        // Four rates fit; the rest are elided rather than dropped silently.
        assert!(label.ends_with("44.1 kHz…"), "{label}");
    }

    #[test]
    fn a_fractional_rate_keeps_its_fraction() {
        // Integer division would print 44.1 kHz as "44 kHz", misreporting a real rate.
        assert_eq!(khz(44_100), "44.1 kHz");
        assert_eq!(khz(48_000), "48 kHz");
        assert_eq!(khz(192_000), "192 kHz");
        assert_eq!(khz(22_050), "22.1 kHz"); // 22.05 rounds to 22.1
    }
}
