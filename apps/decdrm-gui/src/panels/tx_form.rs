//! Form view of the station configuration (the Transmitter tab's "Station" view).
//!
//! The form edits the TOML document in place with `toml_edit`: a value the form changes
//! keeps its trailing comment, and everything the form does not show (alternative
//! frequencies, EPG programmes, …) stays exactly as written. The "TOML" view shows the
//! same text, which is what gets saved and transmitted.

use super::meter::{MeterKind, meter_with_text};
use super::source::DeviceLists;
use decdrm_core::fac::{LANGUAGES, PROGRAMME_TYPES};
use decdrm_core::params::{ChannelLayout, RobustnessMode, SpectrumOccupancy};
use decdrm_core::tx::output::{OutputFormat, recommended_if_range_hz, suggested_if_hz};
use eframe::egui::{self, Color32, RichText, Ui};
use std::path::Path;
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// What the form needs besides the document.
pub struct FormCtx<'a> {
    pub devices: &'a mut DeviceLists,
    /// Directory relative paths in the configuration are resolved against.
    pub base_dir: &'a Path,
    /// The multiplex of the last successful check, for the capacity bar.
    pub bar: Option<&'a PlanBar>,
    /// Input levels (RMS, peak dBFS) of the services while transmitting, by position.
    pub levels: Vec<Option<(f32, f32)>>,
}

/// The multiplex as a bar: MSC capacity and the streams that fill it.
#[derive(Debug, Clone, Default)]
pub struct PlanBar {
    /// MSC bytes per transmission frame.
    pub capacity: usize,
    /// Duration of one transmission frame, ms: 400 for modes A-D, **100 for mode E**. The
    /// bit rate is `capacity * 8 / frame_ms`, so using a constant 400 for mode E reports a
    /// quarter of the real rate.
    pub frame_ms: u32,
    pub segments: Vec<Segment>,
}

/// One stream of the bar.
#[derive(Debug, Clone)]
pub struct Segment {
    pub label: String,
    pub bytes: usize,
    pub audio: bool,
    /// Carried in part A, the higher protected part.
    pub part_a: bool,
    /// The audio service (position in the configuration) whose stream this is.
    pub service: Option<usize>,
}

impl PlanBar {
    /// The bar of a checked configuration.
    pub fn of(cfg: &decdrm_station::StationConfig, plan: &decdrm_station::MultiplexPlan) -> Self {
        let frame_ms = cfg.channel.mode.0.frame_ms();
        let c = &plan.capacity;
        let capacity = (c.vspp_bits + c.hpp_bits + c.lpp_bits) / 8;
        let label_of = |service: usize| cfg.services.get(service).map_or_else(|| format!("service {service}"), |s| s.label.clone());
        let segments = plan
            .streams
            .iter()
            .map(|s| {
                let (bytes, part_a) = (s.bytes(), s.lengths.part_a > 0);
                match &s.content {
                    decdrm_station::StreamContent::Audio { service } => {
                        let label = format!("{} audio", label_of(*service));
                        Segment { label, bytes, audio: true, part_a, service: Some(*service) }
                    }
                    decdrm_station::StreamContent::Data { apps, .. } => {
                        let names: Vec<String> = apps
                            .iter()
                            .filter_map(|a| cfg.services.get(a.service).and_then(|sv| sv.applications().nth(a.index)))
                            .map(|app| format!("{:?}", app.kind).to_lowercase())
                            .collect();
                        Segment { label: names.join(" + "), bytes, audio: false, part_a, service: None }
                    }
                }
            })
            .collect();
        Self { capacity, frame_ms, segments }
    }

    /// Bit rate of the audio stream of service `service` (position), kbit/s.
    pub fn audio_kbps(&self, service: usize) -> Option<f64> {
        // Bytes per transmission frame: * 8 bits / frame duration (400 ms for modes A-D,
        // 100 ms for mode E).
        let ms = f64::from(self.frame_ms.max(1));
        self.segments.iter().find(|s| s.service == Some(service)).map(|s| s.bytes as f64 * 8.0 / ms)
    }
}

/// Draw the form; returns whether the document changed.
pub fn show(ui: &mut Ui, doc: &mut DocumentMut, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    if let Some(bar) = ctx.bar {
        capacity_bar(ui, bar);
        ui.add_space(6.0);
    }
    let mdi = doc.get("mdi").is_some_and(Item::is_table_like);
    card(ui, "Modulator", "MDI from a content server", |ui| changed |= modulator(ui, doc, ctx));
    // A modulator takes the channel, the services and the clock from the MDI.
    if !mdi {
        card(ui, "Channel", "how the signal is built", |ui| changed |= channel(ui, doc));
        changed |= services(ui, doc, ctx);
    }
    card(ui, "Output", "level and format of the signal (the toolbar picks file or sound card)", |ui| changed |= output(ui, doc, ctx));
    card(ui, "Channel simulator", "impair the signal to test receivers", |ui| changed |= simulator(ui, doc));
    if !mdi {
        card(ui, "Clock", "SDC time and date", |ui| changed |= clock(ui, doc));
    }
    ui.label(
        RichText::new("Alternative frequencies, EPG programmes and other details: TOML view.")
            .weak()
            .italics(),
    );
    changed
}

// ---------------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------------

/// A framed section with a title (also used by the status panel).
pub fn card(ui: &mut Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(10)).corner_radius(egui::CornerRadius::same(6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(title).strong().size(15.0));
            ui.label(RichText::new(subtitle).weak());
        });
        ui.add_space(4.0);
        add(ui);
    });
    ui.add_space(8.0);
}

/// A two-column grid (labels left, controls right).
fn grid(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([18.0, 6.0]).min_col_width(130.0).show(ui, add);
}

fn row_label(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).weak())
}

/// A combo box over `(value, label)` options; returns the new value when changed.
fn combo<'a>(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, current: &str, options: &'a [(&'a str, &'a str)], width: f32) -> Option<&'a str> {
    combo_where(ui, id, current, options, width, |_| true, "")
}

/// [`combo`] with the options `allowed` rejects greyed out, explained by `why_not`.
fn combo_where<'a>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
    options: &'a [(&'a str, &'a str)],
    width: f32,
    allowed: impl Fn(&str) -> bool,
    why_not: &str,
) -> Option<&'a str> {
    let shown = options.iter().find(|(v, _)| canon(v) == canon(current)).map_or(current, |(_, l)| l);
    let mut picked = None;
    egui::ComboBox::from_id_salt(id).width(width).selected_text(shown).show_ui(ui, |ui| {
        for (v, l) in options {
            let option = egui::Button::selectable(canon(v) == canon(current), *l);
            if ui.add_enabled(allowed(v), option).on_disabled_hover_text(why_not).clicked() && canon(v) != canon(current) {
                picked = Some(*v);
            }
        }
    });
    picked
}

/// Name matching as the station configuration does it: case-insensitive, ignoring
/// "-", "_" and spaces, with "QAM64" = "64-QAM".
fn canon(s: &str) -> String {
    let t: String = s.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).flat_map(char::to_lowercase).collect();
    match t.strip_prefix("qam") {
        Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => format!("{digits}qam"),
        _ => t,
    }
}

// ---------------------------------------------------------------------------------
// Document helpers (work on standard and inline tables alike)
// ---------------------------------------------------------------------------------

fn get_str(t: &dyn TableLike, k: &str) -> Option<String> {
    t.get(k)?.as_str().map(str::to_string)
}

fn get_int(t: &dyn TableLike, k: &str) -> Option<i64> {
    t.get(k)?.as_integer()
}

/// The working sample rate in kHz from the channel section, defaulting to 48.
///
/// The field is a **string** ("192kHz"), because `decdrm_station`'s `RateSetting` parses and
/// writes it in its own spelling; an integer in the file is rejected by the station parser. This
/// accepts both spellings so a hand-written `120` still reads, and both a string and an integer
/// so a file written by the older form does not panic.
fn sample_rate_khz(ch: &dyn TableLike) -> i64 {
    let Some(item) = ch.get("working_sample_rate") else {
        return 48;
    };
    if let Some(s) = item.as_str() {
        let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
        // "192kHz" and "192" are kHz; a bare "192000" is Hz.
        let n: i64 = digits.parse().unwrap_or(48);
        return if n >= 1000 { n / 1000 } else { n };
    }
    // An integer written by an older build of this form is in kHz.
    item.as_integer().unwrap_or(48)
}

fn get_num(t: &dyn TableLike, k: &str) -> Option<f64> {
    let item = t.get(k)?;
    item.as_float().or_else(|| item.as_integer().map(|i| i as f64))
}

fn get_bool(t: &dyn TableLike, k: &str) -> Option<bool> {
    t.get(k)?.as_bool()
}

/// Set a value, keeping the comments around an existing one.
fn set(t: &mut dyn TableLike, k: &str, v: impl Into<Value>) {
    let v = v.into();
    match t.get_mut(k) {
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *old = v;
            *old.decor_mut() = decor;
        }
        _ => {
            t.insert(k, Item::Value(v));
        }
    }
}

/// Set a value given as TOML source (e.g. a hex integer, which keeps its spelling).
fn set_raw(t: &mut dyn TableLike, k: &str, raw: &str) {
    if let Ok(v) = raw.parse::<Value>() {
        set(t, k, v);
    }
}

fn remove(t: &mut dyn TableLike, k: &str) {
    t.remove(k);
}

/// A child table (standard or inline), created as a standard table if missing.
fn child<'a>(t: &'a mut dyn TableLike, k: &str) -> &'a mut dyn TableLike {
    if !t.get(k).is_some_and(Item::is_table_like) {
        t.insert(k, Item::Table(Table::new()));
    }
    t.get_mut(k).and_then(Item::as_table_like_mut).expect("just made a table")
}

/// A top-level table of the document, created if missing.
fn section<'a>(doc: &'a mut DocumentMut, k: &str) -> &'a mut dyn TableLike {
    child(doc.as_table_mut(), k)
}

// ---------------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------------

/// Robustness modes. Mode E (DRM+) is listed last: it needs a working sample rate above
/// ~94 kHz, so it is offered only when the chosen rate allows it (`RATES` below).
const MODES: &[(&str, &str)] = &[
    ("A", "A — ground wave, few echoes"),
    ("B", "B — sky wave (the usual choice)"),
    ("C", "C — long distance, strong echoes"),
    ("D", "D — severe Doppler and delay"),
    ("E", "E — DRM+ (VHF, 100 kHz: needs 120 or 192 kHz)"),
];
/// Working sample rates of the transmit chain. Modes A–D run at 48 kHz; mode E (DRM+) needs
/// a rate above ~94 kHz, because its 213 carriers reach ±47.11 kHz.
const RATES: &[(&str, &str)] = &[
    ("48", "48 kHz — modes A–D"),
    ("96", "96 kHz — mode E, but only 0.9 kHz of margin: prefer 120"),
    ("120", "120 kHz — mode E (12.9 kHz of margin)"),
    ("192", "192 kHz — mode E (48.9 kHz of margin)"),
];
const OCCUPANCIES: &[(&str, &str)] =
    &[("0", "4.5 kHz"), ("1", "5 kHz"), ("2", "9 kHz"), ("3", "10 kHz"), ("4", "18 kHz"), ("5", "20 kHz")];
const MSC_MODES: &[(&str, &str)] = &[
    ("16-QAM", "16-QAM — robust"),
    ("64-QAM", "64-QAM — more capacity"),
    ("HMsym", "64-QAM hierarchical (HMsym)"),
    ("HMmix", "64-QAM hierarchical (HMmix)"),
];
const SDC_MODES: &[(&str, &str)] = &[("4-QAM", "4-QAM — robust, half the capacity"), ("16-QAM", "16-QAM")];
const INTERLEAVING: &[(&str, &str)] = &[("long", "Long (2 s) — rides out fading"), ("short", "Short (400 ms) — less delay")];
const PROTECTION_16: &[(&str, &str)] = &[("0", "0 — code rate 0.5 (most robust)"), ("1", "1 — code rate 0.62")];
const PROTECTION_64: &[(&str, &str)] =
    &[("0", "0 — code rate 0.5 (most robust)"), ("1", "1 — code rate 0.6"), ("2", "2 — code rate 0.71"), ("3", "3 — code rate 0.78")];
/// Mode E's own four levels, whose code rates are not the DRM30 ones: 16-QAM is table 31
/// (0.33, 0.41, 0.5, 0.62) and 4-QAM table 29 (0.25, 0.33, 0.4, 0.5).
const PROTECTION_E_16: &[(&str, &str)] =
    &[("0", "0 — code rate 0.33 (most robust)"), ("1", "1 — code rate 0.41"), ("2", "2 — code rate 0.5"), ("3", "3 — code rate 0.62")];
const PROTECTION_E_4: &[(&str, &str)] =
    &[("0", "0 — code rate 0.25 (most robust)"), ("1", "1 — code rate 0.33"), ("2", "2 — code rate 0.4"), ("3", "3 — code rate 0.5")];

// ---------------------------------------------------------------------------------
// What robustness mode E changes in the rest of the form
// ---------------------------------------------------------------------------------
//
// Mode E (DRM+) is not a fifth member of the DRM30 family: §7.5.1 gives it its own MSC
// constellations — "16-QAM or 4-QAM mapping in robustness mode E", where modes A–D use
// "64-QAM or 16-QAM" — and §6.3.3 gives its MSC mode field different meanings. The
// selector must therefore change what it offers when the mode changes, and a document
// that came from elsewhere (the TOML view, a hand-edited file, a mode E station saved
// before this existed) can hold a combination the mode cannot express. Both cases go
// through the functions below, so the form and its tests agree on the rules.

/// The MSC constellations `mode` defines, in the order its selector lists them.
///
/// ES 201 980 §7.5.1: "The MSC may use either 64-QAM or 16-QAM mapping in robustness modes
/// A, B, C and D and 16-QAM or 4-QAM mapping in robustness mode E." A choice the mode does
/// not define is **left out of the selector** rather than shown greyed: it is not a choice,
/// and the list the user sees should be the list of real options.
///
/// Mode E lists its two constellations — 16-QAM and 4-QAM — in the order §7.5.1 names them.
/// DecDRM encodes both: mode E's own code-rate tables (29 and 31) are in `tables`, and
/// `MlcParams::msc_for` takes the mode.
pub fn msc_modes_for(mode: &str) -> &'static [(&'static str, &'static str)] {
    if mode == "E" {
        &[("16-QAM", "16-QAM — the DRM+ workhorse"), ("4-QAM", "4-QAM — the robust DRM+ option")]
    } else {
        MSC_MODES
    }
}

/// Whether `msc` names a constellation `mode` defines (and the form can encode).
pub fn msc_mode_allowed(mode: &str, msc: &str) -> bool {
    msc_modes_for(mode).iter().any(|(v, _)| canon(v) == canon(msc))
}

/// The constellation to use for `mode`: `msc` when the mode defines it, else the mode's
/// default — 16-QAM for mode E (what DRM+ transmissions use, §7.5.1), 64-QAM for modes A–D
/// (the station default).
pub fn msc_mode_for(mode: &str, msc: &str) -> &'static str {
    match msc_modes_for(mode).iter().find(|(v, _)| canon(v) == canon(msc)) {
        Some((v, _)) => v,
        None if mode == "E" => "16-QAM",
        None => "64-QAM",
    }
}

/// The SDC constellations `mode` defines. §7.5.2: 16-QAM or 4-QAM at code rate 0.5 for modes
/// A–D, but **4-QAM only** for mode E (at 0.5 or 0.25; DecDRM fixes 0.5, `tables::SDC4_RATE`).
/// Mode E therefore has no choice to offer, and its row is a label rather than a selector.
pub fn sdc_modes_for(mode: &str) -> &'static [(&'static str, &'static str)] {
    if mode == "E" {
        &[("4-QAM", "4-QAM — the only SDC mapping DRM+ has")]
    } else {
        SDC_MODES
    }
}

/// Whether `sdc` names an SDC constellation `mode` defines.
pub fn sdc_mode_allowed(mode: &str, sdc: &str) -> bool {
    sdc_modes_for(mode).iter().any(|(v, _)| canon(v) == canon(sdc))
}

/// The SDC constellation to use for `mode`: `sdc` when the mode defines it, else 4-QAM for
/// mode E (its only mapping) and 16-QAM for modes A–D.
pub fn sdc_mode_for(mode: &str, sdc: &str) -> &'static str {
    match sdc_modes_for(mode).iter().find(|(v, _)| canon(v) == canon(sdc)) {
        Some((v, _)) => v,
        None if mode == "E" => "4-QAM",
        None => "16-QAM",
    }
}

/// The interleaving to use for `mode`: long for mode E (§7.6 — "For robustness mode E only
/// one interleaver depth is available which corresponds to the algorithm for high interleaver
/// depth", its flag fixed at 0, 600 ms), else `il`.
pub fn interleaving_for(mode: &str, il: &str) -> &'static str {
    if mode == "E" {
        "long"
    } else if canon(il) == "short" {
        "short"
    } else {
        "long"
    }
}

/// Protection levels the form offers for a constellation in a mode.
///
/// §7.5.1: 16-QAM has levels 0–1 in modes A–D (table 30) and **0–3 in mode E** (table 31);
/// 4-QAM is mode E only and has 0–3 (table 29); 64-QAM has 0–3 (table 32). `MlcParams::msc_for`
/// uses the mode's own tables, so all four levels of both mode E constellations are real
/// choices, not just the two the DRM30 16-QAM table happens to carry.
pub fn protection_levels(mode: &str, msc: &str) -> &'static [(&'static str, &'static str)] {
    if mode == "E" {
        if canon(msc) == "4qam" { PROTECTION_E_4 } else { PROTECTION_E_16 }
    } else if canon(msc) == "16qam" {
        PROTECTION_16
    } else {
        PROTECTION_64
    }
}

fn channel(ui: &mut Ui, doc: &mut DocumentMut) -> bool {
    // The grid repairs the fields it draws as it draws them. What it cannot reach while its
    // own borrow of `doc` is live — the hierarchical marks on the services — is repaired
    // here, once the borrow is released. The repair is idempotent: a document already in
    // order comes back unchanged, so this costs a comparison per frame and never rewrites a
    // valid document. Running it unconditionally also covers a document *loaded* in a state
    // its mode cannot express (the station's own validation does not reject those), so the
    // form cannot save one either.
    let changed = channel_grid(ui, doc);
    changed | repair_channel_for_mode(doc)
}

/// The Channel card's grid.
fn channel_grid(ui: &mut Ui, doc: &mut DocumentMut) -> bool {
    let mut changed = false;
    let uep = uses_part_a(doc);
    let ch = section(doc, "channel");
    // `mode` is re-read after the selectors that can change it (sample rate and mode), so the
    // rows below draw the mode the document now has rather than the one it had when the frame
    // started. Without that a switch to E drew one frame of DRM30 bandwidth on a DRM+ signal.
    let mut mode = get_str(ch, "mode").unwrap_or_else(|| "B".into());
    let mut msc = get_str(ch, "msc_mode").unwrap_or_else(|| "64-QAM".into());
    let mut hierarchical = canon(&msc).starts_with("hm");
    grid(ui, "tx_form_channel", |ui| {
        row_label(ui, "Sample rate");
        // The value is a **string** in the file ("192kHz"), because `RateSetting` parses it
        // through its own spelling rules ("120", "120kHz" and "192000" all work). Reading it as
        // an integer and writing one back - which this did - produced a file the station parser
        // then rejected.
        let rate = sample_rate_khz(ch).to_string();
        if let Some(v) = combo(ui, "tx_rate", &rate, RATES, 280.0) {
            set(ch, "working_sample_rate", format!("{v}kHz"));
            // Dropping below 100 kHz makes mode E impossible, so fall back to B for the user
            // rather than leave a configuration that cannot validate.
            let hz = v.parse::<i64>().unwrap_or(48) * 1000;
            if hz < 100_000 && mode == "E" {
                set(ch, "mode", "B");
                // Back to DRM30: the mode E choices (16-QAM MSC, 4-QAM SDC, long
                // interleaving) are all valid there, so nothing else needs changing.
            }
            // The rows below must draw the rate just picked, not the previous one.
            mode = get_str(ch, "mode").unwrap_or_else(|| "B".into());
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Robustness mode");
        // Mode E is offered only when the working rate can carry it.
        let rate_hz = sample_rate_khz(ch) * 1000;
        let modes: Vec<(&str, &str)> = MODES
            .iter()
            .copied()
            .filter(|(v, _)| *v != "E" || rate_hz >= 100_000)
            .collect();
        if let Some(v) = combo(ui, "tx_mode", &mode, &modes, 280.0) {
            set(ch, "mode", v);
            mode = v.to_string();
            // Modes C and D only have 10 and 20 kHz; mode E only has occupancy 0 (100 kHz).
            match v {
                "C" | "D" => {
                    if !matches!(get_int(ch, "occupancy"), Some(3 | 5)) {
                        set(ch, "occupancy", 3i64);
                    }
                }
                "E" => set(ch, "occupancy", 0i64),
                _ => {}
            }
            // Mode E changes what the rest of the document may hold (§7.5.1-7.6): its MSC
            // is 16-QAM only in DecDRM, its SDC 4-QAM only, its interleaving long only, and
            // it has no hierarchical layer. Reaching the new mode with the old choices would
            // leave a document the station refuses — the rows below repair the channel and
            // `channel` clears the hierarchical marks. Nothing has to be set here: each row
            // compares the document against the mode's rule and writes only what differs.
            changed = true;
        }
        if mode == "E" {
            ui.label(RichText::new(format!("{:.0} kHz carriers", 2.0 * 47.11)).weak())
                .on_hover_text("Mode E occupies ±47.11 kHz, which is why it needs 120 or 192 kHz");
        }
        ui.end_row();
        row_label(ui, "Bandwidth");
        // Mode E exists at occupancy 0 only (100 kHz, the DRM+ raster), so a selector holding one
        // entry would be noise: show the fixed value instead.
        if mode == "E" {
            set(ch, "occupancy", 0i64);
            ui.label(RichText::new("100 kHz (fixed for DRM+)").weak()).on_hover_text(
                "Robustness mode E is defined at spectrum occupancy 0 only",
            );
            ui.end_row();
        } else {
            let occ = get_int(ch, "occupancy").unwrap_or(3).to_string();
            let allowed: Vec<(&str, &str)> = OCCUPANCIES
                .iter()
                .copied()
                .filter(|(v, _)| !matches!(mode.as_str(), "C" | "D") || matches!(*v, "3" | "5"))
                .collect();
            if let Some(v) = combo(ui, "tx_occupancy", &occ, &allowed, 280.0) {
                set(ch, "occupancy", v.parse::<i64>().unwrap_or(3));
                changed = true;
            }
            ui.end_row();
        }
        row_label(ui, "MSC modulation");
        // The selector lists only what the mode defines (§7.5.1) — a choice that is not an
        // option is not shown — and a document holding one the mode cannot express is
        // repaired to the mode's default as the row is drawn.
        let msc_now = msc_mode_for(&mode, &msc).to_string();
        if get_str(ch, "msc_mode").as_deref() != Some(msc_now.as_str()) {
            set(ch, "msc_mode", msc_now.as_str());
            msc.clone_from(&msc_now);
            // The hierarchical-protection row below reads this, so keep it in step with the
            // constellation actually written.
            hierarchical = canon(&msc).starts_with("hm");
            changed = true;
        }
        let msc_options = msc_modes_for(&mode);
        if msc_options.len() == 1 {
            // One constellation: a selector holding a single entry is noise, so the row shows
            // the value — the same treatment the bandwidth row gets for mode E.
            ui.label(RichText::new(msc_options[0].0).weak())
                .on_hover_text("ES 201 980 §7.5.1 gives robustness mode E 16-QAM or 4-QAM; DecDRM encodes 16-QAM for DRM+");
        } else if let Some(v) = combo(ui, "tx_msc", &msc, msc_options, 280.0) {
            set(ch, "msc_mode", v);
            msc = v.to_string();
            // 16-QAM has protection levels 0-1 only.
            if canon(v) == "16qam" {
                for key in ["protection_b", "protection_a"] {
                    if get_int(ch, key).unwrap_or(0) > 1 {
                        set(ch, key, 1i64);
                    }
                }
            }
            if uep {
                keep_part_a_stronger(ch);
            }
            // Picking a plain constellation drops any hierarchical layer the document still
            // marks; `channel` clears the marks after the grid.
            hierarchical = canon(v).starts_with("hm");
            changed = true;
        }
        ui.end_row();
        row_label(ui, "SDC modulation");
        // §7.5.2: modes A-D have 16-QAM or 4-QAM; mode E has 4-QAM only, at one code rate in
        // DecDRM, so its row is the value rather than a selector holding one entry.
        let sdc = sdc_mode_for(&mode, &get_str(ch, "sdc_mode").unwrap_or_else(|| "16-QAM".into())).to_string();
        if get_str(ch, "sdc_mode").as_deref() != Some(sdc.as_str()) {
            set(ch, "sdc_mode", sdc.as_str());
            changed = true;
        }
        let sdc_options = sdc_modes_for(&mode);
        if sdc_options.len() == 1 {
            ui.label(RichText::new(sdc_options[0].0).weak())
                .on_hover_text("Robustness mode E defines 4-QAM for the SDC only; 16-QAM is DRM30-only");
        } else if let Some(v) = combo(ui, "tx_sdc", &sdc, sdc_options, 280.0) {
            set(ch, "sdc_mode", v);
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Interleaving");
        // §7.6: "For robustness mode E only one interleaver depth is available which
        // corresponds to the algorithm for high interleaver depth" — so mode E's row is the
        // value too, and a document holding short interleaving is repaired to long.
        let il = interleaving_for(&mode, &get_str(ch, "interleaving").unwrap_or_else(|| "long".into())).to_string();
        if get_str(ch, "interleaving").as_deref() != Some(il.as_str()) {
            set(ch, "interleaving", il.as_str());
            changed = true;
        }
        if mode == "E" {
            ui.label(RichText::new("Long").weak())
                .on_hover_text("Robustness mode E has a single interleaver depth, the long algorithm's (600 ms)");
        } else if let Some(v) = combo(ui, "tx_interleaving", &il, INTERLEAVING, 280.0) {
            set(ch, "interleaving", v);
            changed = true;
        }
        ui.end_row();
        let levels = protection_levels(&mode, &msc);
        row_label(ui, "Protection").on_hover_text("Error protection of the multiplex: a lower code rate survives a weaker signal, a higher one carries more");
        let pb = get_int(ch, "protection_b").unwrap_or(1).to_string();
        if let Some(v) = combo(ui, "tx_prot_b", &pb, levels, 280.0) {
            set(ch, "protection_b", v.parse::<i64>().unwrap_or(0));
            if uep {
                keep_part_a_stronger(ch);
            }
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Protection, part A")
            .on_hover_text("For the streams switched to Part A (unequal error protection); it must be more robust than Protection");
        if uep {
            let pb = get_int(ch, "protection_b").unwrap_or(1);
            let pa = get_int(ch, "protection_a").unwrap_or(0).to_string();
            ui.horizontal(|ui| {
                let stronger = |v: &str| v.parse::<i64>().is_ok_and(|level| level < pb);
                let why = "Part A must be protected more strongly (a lower level) than Protection";
                if let Some(v) = combo_where(ui, "tx_prot_a", &pa, levels, 280.0, stronger, why) {
                    set(ch, "protection_a", v.parse::<i64>().unwrap_or(0));
                    changed = true;
                }
                if pb == 0 {
                    let warn = ui.visuals().warn_fg_color;
                    ui.colored_label(warn, "needs Protection above 0");
                }
            });
        } else {
            ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt("tx_prot_a_unused")
                    .width(280.0)
                    .selected_text("not used: no stream is in part A")
                    .show_ui(ui, |_| {})
                    .response
                    .on_disabled_hover_text("Switch a service or data application to Part A to protect it more strongly than the rest");
            });
        }
        ui.end_row();
        if hierarchical {
            row_label(ui, "Hierarchical protection");
            let ph = get_int(ch, "protection_hierarchical").unwrap_or(0).to_string();
            if let Some(v) = combo(ui, "tx_prot_h", &ph, PROTECTION_64, 280.0) {
                set(ch, "protection_hierarchical", v.parse::<i64>().unwrap_or(0));
                changed = true;
            }
            ui.end_row();
        }
    });
    changed
}

/// What the part A switches need while the services are drawn.
struct Parts {
    /// Part B's protection level; at 0 nothing is stronger, so part A is unavailable.
    protection_b: i64,
    /// Switches of named shared data streams: every application of the stream follows.
    shared: Vec<(String, bool)>,
    /// A stream was switched to part A (part A's level may need lowering).
    turned_on: bool,
}

/// How the audio services split the capacity the data applications leave (the
/// station's `share` weights, see `decdrm_station::plan`), and a new share that a
/// service's slider asked for while the services were drawn.
struct Shares {
    /// Weight of each service's audio stream; `None` for data services and for an audio
    /// stream in the hierarchical layer, whose length the channel fixes.
    weights: Vec<Option<f64>>,
    /// Some audio service has an explicit `share`.
    explicit: bool,
    /// Service and its new share in percent.
    request: Option<(usize, f64)>,
}

impl Shares {
    fn of(list: &ArrayOfTables) -> Self {
        let explicit = list.iter().filter_map(|s| s.get("audio")?.as_table_like()).any(|a| a.contains_key("share"));
        Self { weights: list.iter().map(audio_weight).collect(), explicit, request: None }
    }

    /// Service `i`'s part of the audio capacity in percent, when two or more audio
    /// services split it.
    fn percent(&self, i: usize) -> Option<f64> {
        let w = self.weights.get(i).copied().flatten()?;
        let (n, total) = self.weights.iter().flatten().fold((0, 0.0), |(n, t), w| (n + 1, t + w));
        (n >= 2).then(|| 100.0 * w / total)
    }

    /// Weight for an audio service added now: the others' mean, so it gets as much as
    /// an average one (without explicit shares, the default 1).
    fn for_new(&self) -> Option<f64> {
        let (n, total) = self.weights.iter().flatten().fold((0, 0.0), |(n, t), w| (n + 1, t + w));
        (self.explicit && n > 0).then(|| round_share(total / f64::from(n)))
    }
}

/// The share weight of a service's audio stream (see [`Shares::weights`]).
fn audio_weight(svc: &Table) -> Option<f64> {
    let audio = svc.get("audio")?.as_table_like()?;
    if get_bool(audio, "hierarchical").unwrap_or(false) {
        return None;
    }
    Some(get_num(audio, "share").filter(|w| w.is_finite() && *w > 0.0).unwrap_or(1.0))
}

/// The weights, in percent, after service `k`'s share was set to `percent`: the other
/// audio services keep their proportions among themselves (equal parts if they had
/// none). Rounded to 0.1, at least 0.1 (a share must be positive).
fn rebalance(weights: &[Option<f64>], k: usize, percent: f64) -> Vec<Option<f64>> {
    let others: Vec<f64> = weights.iter().enumerate().filter(|&(j, _)| j != k).filter_map(|(_, w)| *w).collect();
    let (n, sum) = (others.len() as f64, others.iter().sum::<f64>());
    let rest = 100.0 - percent;
    weights
        .iter()
        .enumerate()
        .map(|(j, w)| {
            w.map(|w| {
                if j == k {
                    round_share(percent)
                } else if sum > 0.0 {
                    round_share(rest * w / sum)
                } else {
                    round_share(rest / n)
                }
            })
        })
        .collect()
}

fn round_share(w: f64) -> f64 {
    ((w * 10.0).round() / 10.0).max(0.1)
}

/// Write the weights into the services' audio tables.
fn apply_shares(list: &mut ArrayOfTables, weights: &[Option<f64>]) {
    for (svc, w) in list.iter_mut().zip(weights) {
        if let (Some(w), Some(audio)) = (w, svc.get_mut("audio").and_then(Item::as_table_like_mut)) {
            set(audio, "share", *w);
        }
    }
}

fn services(ui: &mut Ui, doc: &mut DocumentMut, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    let protection_b = doc.get("channel").and_then(Item::as_table_like).and_then(|ch| get_int(ch, "protection_b")).unwrap_or(1);
    let mut parts = Parts { protection_b, shared: Vec::new(), turned_on: false };
    if !doc.as_table().get("service").is_some_and(Item::is_array_of_tables) {
        doc.as_table_mut().insert("service", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let list = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).expect("made above");
    let mut shares = Shares::of(list);
    let mut remove_at = None;
    let count = list.len();
    for (i, svc) in list.iter_mut().enumerate() {
        let label = get_str(svc, "label").unwrap_or_default();
        let title = format!("Service {i}");
        egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(10)).corner_radius(egui::CornerRadius::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&title).strong().size(15.0));
                ui.label(RichText::new(&label).size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").on_hover_text("Remove this service").clicked() && confirm_remove(&label) {
                        remove_at = Some(i);
                    }
                });
            });
            ui.add_space(4.0);
            changed |= service(ui, svc, i, ctx, &mut parts, &mut shares);
        });
        ui.add_space(8.0);
    }
    if let Some((k, percent)) = shares.request {
        apply_shares(list, &rebalance(&shares.weights, k, percent));
        changed = true;
    }
    if let Some(i) = remove_at {
        list.remove(i);
        changed = true;
    }
    if count < 4 {
        ui.horizontal(|ui| {
            if ui.button("+ Audio service").clicked() {
                let mut svc = new_service(count, true);
                if let Some(w) = shares.for_new() {
                    set(child(&mut svc, "audio"), "share", w);
                }
                list.push(svc);
                changed = true;
            }
            if ui.button("+ Data service").clicked() {
                list.push(new_service(count, false));
                changed = true;
            }
            ui.label(RichText::new(format!("{count} of 4 services")).weak());
        });
        ui.add_space(8.0);
    }
    for (name, a) in &parts.shared {
        sync_shared_parts(doc, name, *a);
    }
    if parts.turned_on {
        keep_part_a_stronger(section(doc, "channel"));
    }
    changed
}

/// Ask before removing a service (its settings are gone unless the file is reloaded).
fn confirm_remove(label: &str) -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Remove service")
        .set_description(format!("Remove the service \"{label}\" from the station?"))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// A new service with Short Id `n`: an audio service with a test tone, or a data
/// service with Journaline pages.
fn new_service(n: usize, audio: bool) -> Table {
    let mut t = Table::new();
    set(&mut t, "label", if audio { format!("Audio {}", n + 1) } else { format!("Data {}", n + 1) });
    set_raw(&mut t, "id", &format!("0x{:06X}", 0xD0D001 + n as u32));
    if audio {
        let mut a = Table::new();
        set(&mut a, "codec", "he-aac");
        set(&mut a, "core_rate", 12_000i64);
        let mut input = InlineTable::new();
        input.insert("tone_hz", Value::from(1000.0));
        a.insert("input", Item::Value(Value::InlineTable(input)));
        t.insert("audio", Item::Table(a));
    } else {
        let mut d = Table::new();
        set(&mut d, "type", "journaline");
        set(&mut d, "path", "journaline.toml");
        set(&mut d, "bitrate", 1200i64);
        t.insert("data", Item::Table(d));
    }
    t
}

fn service(ui: &mut Ui, svc: &mut Table, i: usize, ctx: &mut FormCtx, parts: &mut Parts, shares: &mut Shares) -> bool {
    let mut changed = false;
    let is_audio = svc.get("audio").is_some_and(Item::is_table_like);
    grid(ui, ("tx_form_service", i), |ui| {
        row_label(ui, "Label");
        let mut label = get_str(svc, "label").unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut label).char_limit(16).desired_width(200.0)).changed() {
                set(svc, "label", label.as_str());
                changed = true;
            }
            ui.label(RichText::new(format!("{}/16", label.chars().count())).weak().small());
        });
        ui.end_row();
        row_label(ui, "Service ID");
        let id = get_int(svc, "id").or_else(|| get_int(svc, "service_id")).unwrap_or(0);
        let key = egui::Id::new(("tx_service_id", i));
        let mut text = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_else(|| format!("{id:06X}"));
        ui.horizontal(|ui| {
            ui.label(RichText::new("0x").monospace().weak());
            let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(80.0).font(egui::TextStyle::Monospace));
            if edit.changed() {
                if let Ok(v) = u32::from_str_radix(text.trim(), 16)
                    && v > 0
                    && v < 1 << 24
                {
                    set_raw(svc, "id", &format!("0x{v:06X}"));
                    changed = true;
                }
                ui.data_mut(|d| d.insert_temp(key, text.clone()));
            }
            if edit.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(key));
            }
            let valid = u32::from_str_radix(text.trim(), 16).is_ok_and(|v| v > 0 && v < 1 << 24);
            if !valid {
                let warn = ui.visuals().warn_fg_color;
                ui.colored_label(warn, "6 hex digits, not 0");
            }
        });
        ui.end_row();
        row_label(ui, "Language");
        let lang = svc.get("language").and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_integer().and_then(|n| LANGUAGES.get(n as usize).map(|s| s.to_string())))).unwrap_or_else(|| "No language specified".into());
        let langs: Vec<(&str, &str)> = LANGUAGES.iter().map(|l| (*l, *l)).collect();
        if let Some(v) = combo(ui, ("tx_lang", i), &lang, &langs, 200.0) {
            set(svc, "language", v);
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Country (ISO)");
        let mut country = get_str(svc, "iso_country").unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut country).char_limit(2).desired_width(36.0).hint_text("gb")).changed() {
                if country.is_empty() {
                    remove(svc, "iso_country");
                } else {
                    set(svc, "iso_country", country.to_lowercase().as_str());
                }
                changed = true;
            }
            ui.label(RichText::new("language (ISO 639-2)").weak());
            let mut iso_lang = get_str(svc, "iso_language").unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut iso_lang).char_limit(3).desired_width(44.0).hint_text("eng")).changed() {
                if iso_lang.is_empty() {
                    remove(svc, "iso_language");
                } else {
                    set(svc, "iso_language", iso_lang.to_lowercase().as_str());
                }
                changed = true;
            }
        });
        ui.end_row();
        if is_audio {
            row_label(ui, "Programme type");
            let pty = svc.get("programme_type").and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_integer().and_then(|n| PROGRAMME_TYPES.get(n as usize).map(|s| s.to_string())))).unwrap_or_else(|| "No programme type".into());
            let ptys: Vec<(&str, &str)> = PROGRAMME_TYPES.iter().map(|p| (*p, *p)).collect();
            if let Some(v) = combo(ui, ("tx_pty", i), &pty, &ptys, 200.0) {
                set(svc, "programme_type", v);
                changed = true;
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    if is_audio {
        let audio = child(svc, "audio");
        changed |= audio_settings(ui, audio, i, ctx, parts, shares);
    } else if svc.get("data").is_some_and(Item::is_table_like) {
        ui.label(RichText::new("Main application").strong());
        let data = child(svc, "data");
        changed |= app_row(ui, data, ("tx_data", i), ctx, parts);
    }
    changed |= extra_apps(ui, svc, i, ctx, parts);
    changed
}

const CODECS: &[(&str, &str)] = &[
    ("aac", "AAC"),
    ("he-aac", "HE-AAC (AAC + SBR)"),
    ("he-aac-v2", "HE-AAC v2 (SBR + parametric stereo)"),
    ("xhe-aac", "xHE-AAC (USAC)"),
    ("opus", "Opus"),
    ("dac", "DAC (neural codec)"),
];
const CORE_RATES: &[(&str, &str)] = &[("12000", "12 kHz core (5 frames per 400 ms)"), ("24000", "24 kHz core (10 frames)")];
const XHE_RATES: &[(&str, &str)] = &[
    ("auto", "Automatic (from the bit rate)"),
    ("9600", "9.6 kHz"),
    ("12000", "12 kHz"),
    ("16000", "16 kHz"),
    ("19200", "19.2 kHz"),
    ("24000", "24 kHz"),
    ("32000", "32 kHz"),
    ("38400", "38.4 kHz (mono)"),
    ("48000", "48 kHz"),
];
const DAC_RATES: &[(&str, &str)] =
    &[("auto", "Highest that fits"), ("1.5", "1.5 kbit/s"), ("3", "3 kbit/s"), ("6", "6 kbit/s"), ("12", "12 kbit/s"), ("24", "24 kbit/s")];
/// Bit rates of a DAC joint stereo service's side signal.
const DAC_SIDE_RATES: &[(&str, &str)] =
    &[("auto", "An eighth of the mid"), ("1.5", "1.5 kbit/s"), ("3", "3 kbit/s"), ("6", "6 kbit/s"), ("12", "12 kbit/s"), ("24", "24 kbit/s")];

/// Which kind of audio input a table describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Tone,
    File,
    LineIn,
    Stream,
}

impl InputKind {
    const ALL: [InputKind; 4] = [InputKind::Tone, InputKind::File, InputKind::LineIn, InputKind::Stream];

    fn of(t: &dyn TableLike) -> Self {
        if t.contains_key("file") {
            InputKind::File
        } else if t.contains_key("device") {
            InputKind::LineIn
        } else if t.contains_key("url") {
            InputKind::Stream
        } else {
            InputKind::Tone
        }
    }

    fn label(self) -> &'static str {
        match self {
            InputKind::Tone => "Test tone",
            InputKind::File => "File",
            InputKind::LineIn => "Line in",
            InputKind::Stream => "Web stream",
        }
    }
}

fn audio_settings(
    ui: &mut Ui,
    audio: &mut dyn TableLike,
    i: usize,
    ctx: &mut FormCtx,
    parts: &mut Parts,
    shares: &mut Shares,
) -> bool {
    let mut changed = false;
    let codec = get_str(audio, "codec").unwrap_or_else(|| "he-aac".into());
    let c = canon(&codec);
    let codecs: Vec<(&str, &str)> =
        CODECS.iter().copied().filter(|(v, _)| *v != "dac" || cfg!(feature = "dac") || canon(v) == c).collect();
    grid(ui, ("tx_form_audio", i), |ui| {
        row_label(ui, "Codec");
        if let Some(v) = combo(ui, ("tx_codec", i), &codec, &codecs, 280.0) {
            set(audio, "codec", v);
            // Keys of the old codec that the new one does not take.
            let aac = matches!(canon(v).as_str(), "aac" | "heaac" | "heaacv2");
            if !aac {
                remove(audio, "core_rate");
            } else if !audio.contains_key("core_rate") {
                set(audio, "core_rate", 12_000i64);
            }
            if canon(v) != "xheaac" {
                remove(audio, "sample_rate");
                remove(audio, "sbr_ratio");
            }
            if canon(v) != "dac" {
                remove(audio, "bandwidth_kbps");
                remove(audio, "side_bandwidth_kbps");
            }
            changed = true;
        }
        ui.end_row();
        match c.as_str() {
            "aac" | "heaac" | "heaacv2" => {
                row_label(ui, "Core rate");
                let rate = get_int(audio, "core_rate").unwrap_or(12_000).to_string();
                if let Some(v) = combo(ui, ("tx_core", i), &rate, CORE_RATES, 280.0) {
                    set(audio, "core_rate", v.parse::<i64>().unwrap_or(12_000));
                    changed = true;
                }
                ui.end_row();
            }
            "xheaac" => {
                row_label(ui, "Sampling rate");
                let rate = get_int(audio, "sample_rate").map_or_else(|| "auto".into(), |r| r.to_string());
                if let Some(v) = combo(ui, ("tx_xhe_rate", i), &rate, XHE_RATES, 280.0) {
                    if v == "auto" {
                        remove(audio, "sample_rate");
                    } else {
                        set(audio, "sample_rate", v.parse::<i64>().unwrap_or(24_000));
                    }
                    changed = true;
                }
                ui.end_row();
            }
            "dac" => {
                row_label(ui, "Bit rate");
                let rate = get_num(audio, "bandwidth_kbps").map_or_else(|| "auto".into(), |r| format!("{r}"));
                if let Some(v) = combo(ui, ("tx_dac_rate", i), &rate, DAC_RATES, 280.0) {
                    if v == "auto" {
                        remove(audio, "bandwidth_kbps");
                    } else {
                        set(audio, "bandwidth_kbps", v.parse::<f64>().unwrap_or(6.0));
                    }
                    changed = true;
                }
                ui.end_row();
            }
            _ => {}
        }
        if c != "heaacv2" {
            row_label(ui, "Channels");
            let mut stereo = get_bool(audio, "stereo").unwrap_or(false);
            let label = if c == "dac" { "Joint stereo" } else { "Stereo" };
            let r = ui.checkbox(&mut stereo, label);
            let r = if c == "dac" {
                r.on_hover_text(
                    "Codes the mid signal (L + R) / 2 at the bit rate above and the side signal (L − R) / 2 at a \
                     lower one; receivers decode two DAC streams, about twice the CPU of mono",
                )
            } else {
                r
            };
            if r.changed() {
                set(audio, "stereo", stereo);
                if !stereo {
                    remove(audio, "side_bandwidth_kbps");
                }
                changed = true;
            }
            ui.end_row();
            if c == "dac" && stereo {
                row_label(ui, "Side bit rate");
                let rate = get_num(audio, "side_bandwidth_kbps").map_or_else(|| "auto".into(), |r| format!("{r}"));
                if let Some(v) = combo(ui, ("tx_dac_side_rate", i), &rate, DAC_SIDE_RATES, 280.0) {
                    if v == "auto" {
                        remove(audio, "side_bandwidth_kbps");
                    } else {
                        set(audio, "side_bandwidth_kbps", v.parse::<f64>().unwrap_or(1.5));
                    }
                    changed = true;
                }
                ui.end_row();
            }
        }
        if let Some(percent) = shares.percent(i) {
            row_label(ui, "Audio share").on_hover_text(
                "This service's part of the capacity the data applications leave; the other audio services keep \
                 their proportions among themselves (the station file's `share`)",
            );
            ui.horizontal(|ui| {
                let mut p = percent.round();
                if ui.add(egui::Slider::new(&mut p, 1.0..=99.0).integer().suffix(" %")).changed() {
                    shares.request = Some((i, p));
                }
                // The rate of the last check (moving the slider checks again).
                if let Some(kbps) = ctx.bar.and_then(|b| b.audio_kbps(i)) {
                    ui.label(RichText::new(format!("{kbps:.1} kbit/s stream")).weak());
                }
            });
            ui.end_row();
        }
        row_label(ui, "Protection");
        changed |= part_switch(ui, audio, "Part A (stronger protection)", parts);
        ui.end_row();
        row_label(ui, "Text messages").on_hover_text("One message per line (up to 128 bytes each), sent one after the other");
        let mut text = audio
            .get("text")
            .and_then(Item::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(2).desired_width(320.0).hint_text("One message per line")).changed() {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            if lines.is_empty() {
                remove(audio, "text");
            } else {
                let mut arr = toml_edit::Array::new();
                for l in lines {
                    arr.push(l);
                }
                set(audio, "text", Value::Array(arr));
            }
            changed = true;
        }
        ui.end_row();
    });
    ui.add_space(6.0);
    let input = child(audio, "input");
    changed |= input_settings(ui, input, i, ctx);
    changed
}

fn input_settings(ui: &mut Ui, input: &mut dyn TableLike, i: usize, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    let kind = InputKind::of(input);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Audio input").strong());
        // While transmitting: the level going into the encoder.
        if let Some(level) = ctx.levels.get(i).copied().flatten() {
            meter_with_text(ui, ("tx_form_level", i), Some(level), 140.0, MeterKind::Audio);
        }
        for k in InputKind::ALL {
            if ui.selectable_label(kind == k, k.label()).clicked() && kind != k {
                for key in ["file", "device", "url", "tone_hz", "loop", "stream_titles"] {
                    remove(input, key);
                }
                match k {
                    InputKind::Tone => set(input, "tone_hz", 1000.0),
                    InputKind::File => set(input, "file", "programme.flac"),
                    InputKind::LineIn => set(input, "device", "default"),
                    InputKind::Stream => set(input, "url", "http://"),
                }
                changed = true;
            }
        }
    });
    grid(ui, ("tx_form_input", i), |ui| match kind {
        InputKind::Tone => {
            row_label(ui, "Frequency");
            let mut f = get_num(input, "tone_hz").unwrap_or(1000.0);
            if ui.add(egui::DragValue::new(&mut f).range(20.0..=20_000.0).speed(10.0).suffix(" Hz")).changed() {
                set(input, "tone_hz", f);
                changed = true;
            }
            ui.end_row();
            row_label(ui, "Level");
            let mut l = get_num(input, "level_dbfs").unwrap_or(-12.0);
            if ui.add(egui::DragValue::new(&mut l).range(-60.0..=0.0).speed(0.5).suffix(" dBFS")).changed() {
                set(input, "level_dbfs", l);
                changed = true;
            }
            ui.end_row();
        }
        InputKind::File => {
            row_label(ui, "File");
            let mut path = get_str(input, "file").unwrap_or_default();
            ui.horizontal(|ui| {
                if ui.add(egui::TextEdit::singleline(&mut path).desired_width(260.0).hint_text("WAV or FLAC")).changed() {
                    set(input, "file", path.as_str());
                    changed = true;
                }
                if ui.button("Browse…").clicked()
                    && let Some(p) = pick_file(ctx.base_dir, "Audio file", &["wav", "flac"])
                {
                    set(input, "file", p.as_str());
                    changed = true;
                }
            });
            ui.end_row();
            row_label(ui, "At the end");
            let mut looped = get_bool(input, "loop").unwrap_or(true);
            if ui.checkbox(&mut looped, "Start again (loop)").changed() {
                set(input, "loop", looped);
                changed = true;
            }
            ui.end_row();
            changed |= gain_row(ui, input);
        }
        InputKind::LineIn => {
            row_label(ui, "Sound card");
            let current = get_str(input, "device").unwrap_or_else(|| "default".into());
            ui.horizontal(|ui| {
                let shown = if current == "default" { "Default input".to_string() } else { current.clone() };
                egui::ComboBox::from_id_salt(("tx_line_in", i)).width(260.0).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(current == "default", "Default input").clicked() {
                        set(input, "device", "default");
                        changed = true;
                    }
                    for d in ctx.devices.inputs().to_vec() {
                        if ui.selectable_label(current == d.name, d.label()).clicked() {
                            set(input, "device", d.name.as_str());
                            changed = true;
                        }
                    }
                });
                if ui.small_button("⟳").on_hover_text("Refresh the device list").clicked() {
                    ctx.devices.refresh();
                }
            });
            ui.end_row();
            changed |= gain_row(ui, input);
        }
        InputKind::Stream => {
            row_label(ui, "Stream URL");
            let mut url = get_str(input, "url").unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut url).desired_width(320.0).hint_text("http://… (Icecast/Shoutcast, MP3, AAC, Ogg; or a .m3u/.pls)"))
                .changed()
            {
                set(input, "url", url.trim());
                changed = true;
            }
            ui.end_row();
            row_label(ui, "Titles");
            let mut titles = get_bool(input, "stream_titles").unwrap_or(true);
            if ui.checkbox(&mut titles, "Send the stream's \"now playing\" as text messages").changed() {
                set(input, "stream_titles", titles);
                changed = true;
            }
            ui.end_row();
            changed |= gain_row(ui, input);
        }
    });
    changed
}

fn gain_row(ui: &mut Ui, input: &mut dyn TableLike) -> bool {
    row_label(ui, "Gain");
    let mut g = get_num(input, "gain_db").unwrap_or(0.0);
    let changed = ui.add(egui::DragValue::new(&mut g).range(-40.0..=40.0).speed(0.25).suffix(" dB")).changed();
    if changed {
        if g == 0.0 {
            remove(input, "gain_db");
        } else {
            set(input, "gain_db", g);
        }
    }
    ui.end_row();
    changed
}

const APP_KINDS: &[(&str, &str)] = &[
    ("slideshow", "MOT slideshow"),
    ("website", "Broadcast website"),
    ("journaline", "Journaline"),
    ("epg", "Programme guide (EPG)"),
    ("tpeg", "TPEG"),
    ("raw", "Raw data"),
];

const JOURNALINE_SOURCES: &[(&str, &str)] = &[("file", "Page file"), ("feeds", "RSS feeds")];

/// Keys of a Journaline application's feeds.
const FEED_KEYS: [&str; 4] = ["feeds", "feed_refresh", "feed_items", "feed_title"];

/// One data application: type, file or folder (Journaline: or RSS feeds), bit rate,
/// part A.
fn app_row(ui: &mut Ui, app: &mut dyn TableLike, id: impl std::hash::Hash + std::fmt::Debug + Copy, ctx: &mut FormCtx, parts: &mut Parts) -> bool {
    let mut changed = false;
    let kind = get_str(app, "type").unwrap_or_else(|| "journaline".into());
    let journaline = canon(&kind) == "journaline";
    // A Journaline application with a `feeds` key (even an empty one) takes feeds.
    let feeds = journaline && app.contains_key("feeds");
    ui.horizontal_wrapped(|ui| {
        if let Some(v) = combo(ui, (id, "kind"), &kind, APP_KINDS, 170.0) {
            set(app, "type", v);
            if canon(v) != "journaline" {
                FEED_KEYS.iter().for_each(|k| remove(app, k));
            }
            changed = true;
        }
        if journaline
            && let Some(v) = combo(ui, (id, "source"), if feeds { "feeds" } else { "file" }, JOURNALINE_SOURCES, 100.0)
        {
            if v == "feeds" {
                remove(app, "path");
                set(app, "feeds", toml_edit::Array::new());
            } else {
                FEED_KEYS.iter().for_each(|k| remove(app, k));
            }
            changed = true;
        }
        if !feeds {
            changed |= path_editor(ui, app, &kind, ctx);
        }
        let mut rate = get_int(app, "bitrate").unwrap_or(2000);
        if ui.add(egui::DragValue::new(&mut rate).range(100..=64_000).speed(50.0).suffix(" bit/s")).changed() {
            set(app, "bitrate", rate);
            changed = true;
        }
        changed |= part_switch(ui, app, "Part A", parts);
    });
    if feeds {
        changed |= feed_settings(ui, app, id);
    }
    changed
}

/// The file or folder of a data application.
fn path_editor(ui: &mut Ui, app: &mut dyn TableLike, kind: &str, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    let mut path = get_str(app, "path").unwrap_or_default();
    let hint = match canon(kind).as_str() {
        "slideshow" => "folder of images",
        "website" => "site folder",
        "journaline" => "page file (.toml)",
        "epg" => "schedule (.toml, optional)",
        _ => "data file",
    };
    if ui.add(egui::TextEdit::singleline(&mut path).desired_width(200.0).hint_text(hint)).changed() {
        if path.is_empty() {
            remove(app, "path");
        } else {
            set(app, "path", path.as_str());
        }
        changed = true;
    }
    if ui.small_button("…").on_hover_text("Choose").clicked() {
        let picked = if matches!(canon(kind).as_str(), "slideshow" | "website") {
            pick_folder(ctx.base_dir)
        } else {
            pick_file(ctx.base_dir, "Data", &["toml", "json", "bin", "*"])
        };
        if let Some(p) = picked {
            set(app, "path", p.as_str());
            changed = true;
        }
    }
    changed
}

/// The feed URLs of a Journaline application.
fn get_feeds(app: &dyn TableLike) -> Vec<String> {
    let Some(list) = app.get("feeds").and_then(Item::as_array) else { return Vec::new() };
    list.iter().filter_map(|v| v.as_str()).map(str::to_string).collect()
}

/// The URLs in the feed box: one per line, blank lines left out.
fn feed_lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

const FEEDS_HELP: &str = "RSS or Atom feeds (http:// or https://), one per line. The root menu lists the feeds, each \
     feed's menu its items, each item's page its description and link. The feeds are fetched when the \
     transmission starts and then on the schedule; changed pages go out at once with the next revision.";

/// A Journaline application's feeds: the URLs, how often they are fetched, how many
/// items each, and the root menu's title.
fn feed_settings(ui: &mut Ui, app: &mut dyn TableLike, id: impl std::hash::Hash + std::fmt::Debug + Copy) -> bool {
    let mut changed = false;
    let feeds = get_feeds(app);
    // The box's text is kept while it means the same URLs (so a new empty line can be
    // typed); otherwise it is made from the file.
    let key = egui::Id::new((id, "feed_text"));
    let mut text = ui
        .data(|d| d.get_temp::<String>(key))
        .filter(|t| feed_lines(t) == feeds)
        .unwrap_or_else(|| feeds.join("\n"));
    let edit = egui::TextEdit::multiline(&mut text)
        .desired_rows(2)
        .desired_width(f32::INFINITY)
        .hint_text("https://… one feed per line");
    if ui.add(edit).on_hover_text(FEEDS_HELP).changed() {
        let lines = feed_lines(&text);
        if lines != feeds {
            set(app, "feeds", lines.iter().map(String::as_str).collect::<toml_edit::Array>());
            changed = true;
        }
    }
    ui.data_mut(|d| d.insert_temp(key, text));
    ui.horizontal_wrapped(|ui| {
        ui.label("Every");
        let mut minutes = get_int(app, "feed_refresh").unwrap_or(600) / 60;
        if ui
            .add(egui::DragValue::new(&mut minutes).range(1..=1440).suffix(" min"))
            .on_hover_text("How often the feeds are fetched (also: Update while transmitting)")
            .changed()
        {
            if minutes == 10 {
                remove(app, "feed_refresh");
            } else {
                set(app, "feed_refresh", minutes * 60);
            }
            changed = true;
        }
        let mut items = get_int(app, "feed_items").unwrap_or(10);
        if ui
            .add(egui::DragValue::new(&mut items).range(1..=50).suffix(" items"))
            .on_hover_text("Items per feed, the first ones of each")
            .changed()
        {
            if items == 10 {
                remove(app, "feed_items");
            } else {
                set(app, "feed_items", items);
            }
            changed = true;
        }
        let mut title = get_str(app, "feed_title").unwrap_or_default();
        if ui
            .add(egui::TextEdit::singleline(&mut title).desired_width(140.0).hint_text("News feeds"))
            .on_hover_text("Title of the root menu")
            .changed()
        {
            if title.trim().is_empty() {
                remove(app, "feed_title");
            } else {
                set(app, "feed_title", title.as_str());
            }
            changed = true;
        }
    });
    changed
}

const PART_A_HELP: &str = "Send this stream in part A, coded at the Channel card's \"Protection, part A\" instead of \
     \"Protection\" (unequal error protection). Part A bytes take more of the channel, so the other streams get less.";

/// The part A switch of one stream (an audio service or a data application).
fn part_switch(ui: &mut Ui, t: &mut dyn TableLike, text: &str, parts: &mut Parts) -> bool {
    if get_bool(t, "hierarchical").unwrap_or(false) {
        ui.label(RichText::new("hierarchical layer").weak())
            .on_hover_text("This stream is in the hierarchical layer (TOML view), which has its own protection level");
        return false;
    }
    let mut on = is_part_a(t);
    // At part B's most robust level nothing is stronger; switching off always works.
    let possible = on || parts.protection_b > 0;
    let r = ui
        .add_enabled(possible, egui::Checkbox::new(&mut on, text))
        .on_hover_text(PART_A_HELP)
        .on_disabled_hover_text("Protection is already at its most robust level (0): choose a higher Protection in the Channel card first");
    if !r.changed() {
        return false;
    }
    set_part(t, on);
    if let Some(name) = get_str(t, "stream") {
        parts.shared.push((name, on));
    }
    parts.turned_on |= on;
    true
}

/// Whether a stream table sets `part = "A"` (spelled as the station accepts it).
fn is_part_a(t: &dyn TableLike) -> bool {
    get_str(t, "part").is_some_and(|p| matches!(canon(&p).as_str(), "a" | "higher" | "high"))
}

/// Put a stream into part A, or back into part B (the default, so the key goes).
fn set_part(t: &mut dyn TableLike, a: bool) {
    if a {
        set(t, "part", "A");
    } else {
        remove(t, "part");
    }
}

/// Whether any stream is in part A: an audio service or data application with
/// `part = "A"` that is not in the hierarchical layer (which has no part).
fn uses_part_a(doc: &DocumentMut) -> bool {
    let in_a = |t: &dyn TableLike| is_part_a(t) && !get_bool(t, "hierarchical").unwrap_or(false);
    doc.get("service").and_then(Item::as_array_of_tables).is_some_and(|list| {
        list.iter().any(|svc| {
            ["audio", "data"].into_iter().any(|k| svc.get(k).and_then(Item::as_table_like).is_some_and(in_a))
                || svc.get("app").and_then(Item::as_array_of_tables).is_some_and(|apps| apps.iter().any(|app| in_a(app)))
        })
    })
}

/// Give every application of the shared stream `name` the same part, as the station
/// requires.
fn sync_shared_parts(doc: &mut DocumentMut, name: &str, a: bool) {
    let Some(list) = doc.get_mut("service").and_then(Item::as_array_of_tables_mut) else { return };
    for svc in list.iter_mut() {
        if let Some(data) = svc.get_mut("data").and_then(Item::as_table_like_mut)
            && get_str(data, "stream").as_deref() == Some(name)
        {
            set_part(data, a);
        }
        if let Some(apps) = svc.get_mut("app").and_then(Item::as_array_of_tables_mut) {
            for app in apps.iter_mut() {
                if get_str(app, "stream").as_deref() == Some(name) {
                    set_part(app, a);
                }
            }
        }
    }
}

/// With a stream in part A, keep part A's level below part B's (part A is the higher
/// protected part), as far as part B leaves room.
fn keep_part_a_stronger(ch: &mut dyn TableLike) {
    let pb = get_int(ch, "protection_b").unwrap_or(1);
    if get_int(ch, "protection_a").unwrap_or(0) >= pb && pb > 0 {
        set(ch, "protection_a", pb - 1);
    }
}

/// Bring the rest of the document into what its robustness mode allows, after the mode
/// changed (or when the document arrived with a combination the mode cannot express).
///
/// The three fields with per-mode rules are the MSC constellation (§7.5.1: mode E has
/// 16-QAM and 4-QAM; A–D have 16-QAM and 64-QAM), the SDC constellation (§7.5.2: mode E
/// has 4-QAM only) and the interleaving depth (§7.6: mode E has the long depth only).
/// Protection levels are clamped to what the chosen constellation defines.
///
/// Hierarchical modulation only exists in 64-QAM (§7.5.1: "a hierarchical mapping scheme can
/// only be used in a 64-QAM signal constellation"), so a mode E document cannot carry a
/// stream marked `hierarchical = true`: the mark is dropped, and the station would otherwise
/// refuse the configuration.
///
/// Returns whether anything changed.
fn repair_channel_for_mode(doc: &mut DocumentMut) -> bool {
    let mut changed = false;
    // Read what the clamping below needs from the whole document first: `ch` borrows `doc`
    // mutably, and `uses_part_a` needs to read the services.
    let part_a = uses_part_a(doc);
    let ch = section(doc, "channel");
    let mode = get_str(ch, "mode").unwrap_or_else(|| "B".into());
    // The comparisons are against the *effective* value (the key, or the default when it is
    // absent) and are canonical, so a document that leaves a key out at its default, or spells
    // it its own way, is not rewritten just for being opened.
    changed |= repair_str(ch, "msc_mode", "64-QAM", |v| msc_mode_for(&mode, v).to_string());
    changed |= repair_str(ch, "sdc_mode", "16-QAM", |v| sdc_mode_for(&mode, v).to_string());
    changed |= repair_str(ch, "interleaving", "long", |v| interleaving_for(&mode, v).to_string());
    // Occupancy: mode E exists at 0 only; C and D only at 3 or 5.
    let occ = get_int(ch, "occupancy").unwrap_or(3);
    let occ_ok = match mode.as_str() {
        "E" => occ == 0,
        "C" | "D" => matches!(occ, 3 | 5),
        _ => true,
    };
    if !occ_ok {
        set(ch, "occupancy", if mode == "E" { 0i64 } else { 3 });
        changed = true;
    }
    // Protection levels above what the constellation defines (§7.5.1 tables 29-32). Only the
    // levels the station actually uses are clamped, as its own validation does: `protection_b`
    // always, `protection_a` only when a stream is in part A, `protection_hierarchical` only
    // when the constellation has a hierarchical layer. A stale value in an unused field is
    // harmless, and rewriting it would change a document the station accepts.
    let msc = get_str(ch, "msc_mode").unwrap_or_else(|| "64-QAM".into());
    let max = protection_levels(&mode, &msc).len() as i64 - 1;
    let mut used = vec![("protection_b", true)];
    used.push(("protection_a", part_a));
    used.push(("protection_hierarchical", canon(&msc).starts_with("hm")));
    for (key, used) in used {
        if used && get_int(ch, key).unwrap_or(0) > max {
            set(ch, key, max);
            changed = true;
        }
    }
    // No hierarchical layer without a 64-QAM constellation.
    if !canon(&msc).starts_with("hm") {
        changed |= clear_hierarchical_marks(doc);
    }
    changed
}

/// Set `key` to `rule(effective)` when that differs from the value the document already has
/// (`default` when the key is absent). Comparison is canonical, so an equivalent spelling is
/// left as written; a key that is absent and whose default already satisfies the rule is not
/// added. Returns whether the document changed.
fn repair_str(ch: &mut dyn TableLike, key: &str, default: &str, rule: impl Fn(&str) -> String) -> bool {
    let effective = get_str(ch, key).unwrap_or_else(|| default.to_string());
    let corrected = rule(&effective);
    if canon(&corrected) == canon(&effective) {
        return false;
    }
    set(ch, key, corrected);
    true
}

/// Drop `hierarchical = true` from every stream (audio services and data applications).
/// A mark that cannot be honoured would make the station refuse the configuration.
/// Returns whether any stream was marked.
fn clear_hierarchical_marks(doc: &mut DocumentMut) -> bool {
    let mut changed = false;
    if let Some(services) = doc.get_mut("service").and_then(Item::as_array_of_tables_mut) {
        for svc in services.iter_mut() {
            for key in ["audio", "data"] {
                if let Some(t) = svc.get_mut(key).and_then(Item::as_table_like_mut) {
                    changed |= take_true(t, "hierarchical");
                }
            }
            if let Some(apps) = svc.get_mut("app").and_then(Item::as_array_of_tables_mut) {
                for app in apps.iter_mut() {
                    changed |= take_true(app, "hierarchical");
                }
            }
        }
    }
    changed
}

/// Remove `key` when it is set to `true`; returns whether it was.
fn take_true(t: &mut dyn TableLike, key: &str) -> bool {
    if !get_bool(t, key).unwrap_or(false) {
        return false;
    }
    remove(t, key);
    true
}

/// The service's extra data applications (`[[service.app]]`).
fn extra_apps(ui: &mut Ui, svc: &mut Table, i: usize, ctx: &mut FormCtx, parts: &mut Parts) -> bool {
    let mut changed = false;
    let has = svc.get("app").is_some_and(Item::is_array_of_tables);
    if has {
        ui.add_space(4.0);
        ui.label(RichText::new(if svc.contains_key("data") { "More applications" } else { "Data applications" }).strong());
        let apps = svc.get_mut("app").and_then(Item::as_array_of_tables_mut).expect("checked");
        let mut remove_at = None;
        for (k, app) in apps.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= app_row(ui, app, ("tx_app", i, k), ctx, parts);
                if ui.small_button("Remove").on_hover_text("Remove this application").clicked() {
                    remove_at = Some(k);
                }
            });
        }
        if let Some(k) = remove_at {
            apps.remove(k);
            if apps.is_empty() {
                svc.remove("app");
            }
            changed = true;
        }
    }
    if ui.small_button("+ Data application").on_hover_text("Add a slideshow, Journaline pages, … to this service").clicked() {
        if !has {
            svc.insert("app", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        let apps = svc.get_mut("app").and_then(Item::as_array_of_tables_mut).expect("made above");
        let mut t = Table::new();
        set(&mut t, "type", "slideshow");
        set(&mut t, "path", "slides");
        set(&mut t, "bitrate", 2000i64);
        apps.push(t);
        changed = true;
    }
    changed
}

const FORMATS: &[(&str, &str)] = &[("real", "Real IF (1 channel)"), ("iq", "I/Q (2 channels)")];
const SAMPLE_FORMATS: &[(&str, &str)] = &[("int16", "16-bit"), ("int24", "24-bit"), ("float32", "32-bit float")];

/// The channel layout (robustness mode and bandwidth) of the document, if valid.
fn channel_layout(doc: &DocumentMut) -> Option<ChannelLayout> {
    let ch = doc.get("channel").and_then(Item::as_table_like);
    let mode = match canon(&ch.and_then(|ch| get_str(ch, "mode")).unwrap_or_else(|| "B".into())).as_str() {
        "a" => RobustnessMode::A,
        "b" => RobustnessMode::B,
        "c" => RobustnessMode::C,
        "d" => RobustnessMode::D,
        "e" | "drm+" => RobustnessMode::E,
        _ => return None,
    };
    let occupancy = u8::try_from(ch.and_then(|ch| get_int(ch, "occupancy")).unwrap_or(3)).ok()?;
    ChannelLayout::new(mode, SpectrumOccupancy::new(occupancy)?)
}

fn output(ui: &mut Ui, doc: &mut DocumentMut, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    let layout = channel_layout(doc);
    let out = section(doc, "output");
    grid(ui, "tx_form_output", |ui| {
        row_label(ui, "Signal");
        let format = get_str(out, "format").unwrap_or_else(|| "real".into());
        if let Some(v) = combo(ui, "tx_format", &format, FORMATS, 200.0) {
            set(out, "format", v);
            changed = true;
        }
        ui.end_row();
        if canon(&format) == "iq" {
            row_label(ui, "DC carrier offset");
            let mut off = get_num(out, "iq_offset_hz").unwrap_or(0.0);
            ui.horizontal(|ui| {
                if ui.add(egui::DragValue::new(&mut off).range(-20_000.0..=20_000.0).speed(10.0).suffix(" Hz")).changed() {
                    set(out, "iq_offset_hz", off);
                    changed = true;
                }
                let mut swap = get_bool(out, "iq_swap").unwrap_or(false);
                if ui.checkbox(&mut swap, "I on the right channel").changed() {
                    set(out, "iq_swap", swap);
                    changed = true;
                }
            });
            ui.end_row();
        } else {
            row_label(ui, "IF (DC carrier)");
            let mut auto = !out.contains_key("if_hz");
            let suggested = layout.map_or(12_000.0, suggested_if_hz);
            ui.horizontal(|ui| {
                let hint = "Centre the signal at 12 kHz: the DC carrier at 12 kHz for 9 and 10 kHz, lower for 4.5 and 5 kHz \
                     (all their carriers lie above it) and for 18 and 20 kHz";
                if ui.checkbox(&mut auto, "Automatic").on_hover_text(hint).changed() {
                    if auto {
                        remove(out, "if_hz");
                    } else {
                        set(out, "if_hz", suggested);
                    }
                    changed = true;
                }
                let mut f = if auto { suggested } else { get_num(out, "if_hz").unwrap_or(suggested) };
                if !auto {
                    // The form offers what keeps the signal clear of 0 Hz and 24 kHz; a
                    // value written in the TOML view stays as it is.
                    let (min, max) = layout.map_or((3_000.0, 21_000.0), recommended_if_range_hz);
                    let edit = egui::DragValue::new(&mut f).range(min..=max).clamp_existing_to_range(false).speed(10.0).suffix(" Hz");
                    if ui.add(edit).changed() {
                        set(out, "if_hz", f);
                        changed = true;
                    }
                }
                if let Some(l) = layout {
                    let (_, (lo, hi)) = crate::tx_config::signal_band(l, OutputFormat::Real { if_hz: f });
                    ui.label(RichText::new(format!("signal {:.1}–{:.1} kHz", lo / 1e3, hi / 1e3)).weak());
                }
            });
            ui.end_row();
        }
        row_label(ui, "Level");
        let mut level = get_num(out, "level_dbfs").unwrap_or(-15.0);
        ui.horizontal(|ui| {
            if ui.add(egui::DragValue::new(&mut level).range(-40.0..=-3.0).speed(0.25).suffix(" dBFS RMS")).changed() {
                set(out, "level_dbfs", level);
                changed = true;
            }
            ui.label(RichText::new("OFDM peaks are ~10 dB higher").weak());
        });
        ui.end_row();
        row_label(ui, "Filter");
        let mut band = get_bool(out, "band_limit").unwrap_or(true);
        if ui.checkbox(&mut band, "Band-limit the signal (against the OFDM side lobes)").changed() {
            set(out, "band_limit", band);
            changed = true;
        }
        ui.end_row();
        row_label(ui, "File (as configured)");
        let mut file = get_str(out, "file").unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut file).desired_width(200.0).hint_text("drm_station.wav")).changed() {
                if file.is_empty() {
                    remove(out, "file");
                } else {
                    set(out, "file", file.as_str());
                }
                changed = true;
            }
            let fmt = get_str(out, "sample_format").unwrap_or_else(|| "int16".into());
            if let Some(v) = combo(ui, "tx_sample_format", &fmt, SAMPLE_FORMATS, 110.0) {
                set(out, "sample_format", v);
                changed = true;
            }
        });
        ui.end_row();
        row_label(ui, "Sound card (as configured)");
        let mut dev = get_str(out, "device").unwrap_or_default();
        ui.horizontal(|ui| {
            let shown = if dev.is_empty() { "none".to_string() } else { dev.clone() };
            egui::ComboBox::from_id_salt("tx_out_device_cfg").width(220.0).selected_text(shown).show_ui(ui, |ui| {
                if ui.selectable_label(dev.is_empty(), "none").clicked() {
                    remove(out, "device");
                    changed = true;
                }
                for d in ctx.devices.outputs().to_vec() {
                    if ui.selectable_label(dev == d.name, d.label()).clicked() {
                        dev.clone_from(&d.name);
                        set(out, "device", d.name.as_str());
                        changed = true;
                    }
                }
            });
            let mut buf = get_int(out, "device_buffer_ms").unwrap_or(400);
            if ui.add(egui::DragValue::new(&mut buf).range(100..=2_000).speed(10.0).prefix("buffer ").suffix(" ms")).changed() {
                set(out, "device_buffer_ms", buf);
                changed = true;
            }
        });
        ui.end_row();
    });
    changed
}

const CHANNEL_MODELS: &[(&str, &str)] = &[
    ("1", "1 — AWGN only"),
    ("2", "2 — Rice with delay"),
    ("3", "3 — US Consortium"),
    ("4", "4 — CCIR poor"),
    ("5", "5 — severe Doppler"),
    ("6", "6 — severe Doppler and delay"),
    ("ra", "VHF RA — rural (COST 207)"),
    ("tu6", "VHF TU6 — typical urban"),
    ("ht", "VHF HT — hilly terrain"),
];

/// Apply a propagation-model choice to `[simulate]`. The VHF presets are a different shape
/// from the annex B number: they replace `channel` with `vhf` plus a speed and a band, so
/// the keys the chosen model does not use are removed rather than left stale (the station's
/// `deny_unknown_fields` would accept them, but they would silently describe another model).
fn set_simulation_model(sim: &mut dyn TableLike, choice: &str) {
    match choice {
        "ra" | "tu6" | "ht" => {
            remove(sim, "channel");
            set(sim, "vhf", choice);
            if get_num(sim, "speed_kmh").is_none() {
                set(sim, "speed_kmh", 0.0);
            }
            if get_num(sim, "carrier_mhz").is_none() {
                set(sim, "carrier_mhz", 200.0);
            }
        }
        number => {
            remove(sim, "vhf");
            remove(sim, "speed_kmh");
            remove(sim, "carrier_mhz");
            set(sim, "channel", number.parse::<i64>().unwrap_or(1));
        }
    }
}

fn simulator(ui: &mut Ui, doc: &mut DocumentMut) -> bool {
    let mut changed = false;
    let mut on = doc.as_table().get("simulate").is_some_and(Item::is_table_like);
    if ui.checkbox(&mut on, "Impair the transmitted signal").changed() {
        if on {
            let mut t = Table::new();
            set(&mut t, "channel", 3i64);
            set(&mut t, "snr_db", 20.0);
            doc.as_table_mut().insert("simulate", Item::Table(t));
        } else {
            doc.as_table_mut().remove("simulate");
        }
        changed = true;
    }
    if !on {
        return changed;
    }
    let sim = section(doc, "simulate");
    grid(ui, "tx_form_sim", |ui| {
        row_label(ui, "Propagation");
        let model = get_str(sim, "vhf").unwrap_or_else(|| get_int(sim, "channel").unwrap_or(1).to_string());
        if let Some(v) = combo(ui, "tx_sim_model", &model, CHANNEL_MODELS, 260.0) {
            set_simulation_model(sim, &v);
            changed = true;
        }
        ui.end_row();
        if get_str(sim, "vhf").is_some() {
            // VHF: the profile's shape depends on the speed (0 = parked, no fading) and on
            // the band (the Doppler shift is v·f/c).
            row_label(ui, "Speed");
            let mut speed = get_num(sim, "speed_kmh").unwrap_or(0.0);
            if ui.add(egui::DragValue::new(&mut speed).range(0.0..=300.0).speed(1.0).suffix(" km/h")).changed() {
                set(sim, "speed_kmh", speed);
                changed = true;
            }
            ui.end_row();
            row_label(ui, "Band");
            let mut mhz = get_num(sim, "carrier_mhz").unwrap_or(200.0);
            if ui.add(egui::DragValue::new(&mut mhz).range(40.0..=300.0).speed(1.0).suffix(" MHz")).changed() {
                set(sim, "carrier_mhz", mhz);
                changed = true;
            }
            ui.end_row();
        }
        row_label(ui, "Noise");
        let mut noisy = sim.contains_key("snr_db");
        ui.horizontal(|ui| {
            if ui.checkbox(&mut noisy, "SNR").changed() {
                if noisy {
                    set(sim, "snr_db", 20.0);
                } else {
                    remove(sim, "snr_db");
                }
                changed = true;
            }
            if noisy {
                let mut snr = get_num(sim, "snr_db").unwrap_or(20.0);
                if ui.add(egui::DragValue::new(&mut snr).range(-5.0..=60.0).speed(0.1).suffix(" dB")).changed() {
                    set(sim, "snr_db", snr);
                    changed = true;
                }
            }
        });
        ui.end_row();
        row_label(ui, "Frequency offset");
        let mut fo = get_num(sim, "frequency_offset_hz").unwrap_or(0.0);
        if ui.add(egui::DragValue::new(&mut fo).range(-500.0..=500.0).speed(0.5).suffix(" Hz")).changed() {
            set(sim, "frequency_offset_hz", fo);
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Clock error");
        let mut sro = get_num(sim, "sample_rate_offset_ppm").unwrap_or(0.0);
        if ui.add(egui::DragValue::new(&mut sro).range(-500.0..=500.0).speed(0.5).suffix(" ppm")).changed() {
            set(sim, "sample_rate_offset_ppm", sro);
            changed = true;
        }
        ui.end_row();
    });
    changed
}

/// `[mdi]`: transmit MDI from a content server instead of the services.
fn modulator(ui: &mut Ui, doc: &mut DocumentMut, ctx: &mut FormCtx) -> bool {
    let mut changed = false;
    let mut on = doc.get("mdi").is_some_and(Item::is_table_like);
    if ui
        .checkbox(&mut on, "Transmit MDI from a content server instead of the services")
        .on_hover_text(
            "DecDRM as a modulator: the multiplex comes over UDP (MDI, ETSI TS 102 820) from a content server \
             (Dream, a commercial one) or from a recording, and is transmitted as it is.",
        )
        .changed()
    {
        if on {
            let t = section(doc, "mdi");
            if get_str(t, "input").is_none() {
                set(t, "input", "8000");
            }
        } else {
            doc.remove("mdi");
        }
        changed = true;
    }
    if !on {
        return changed;
    }
    let t = section(doc, "mdi");
    grid(ui, "tx_form_mdi", |ui| {
        row_label(ui, "Input");
        ui.horizontal(|ui| {
            let mut input = get_str(t, "input").unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut input).desired_width(220.0).hint_text("UDP port, group:port or a recording"))
                .on_hover_text(
                    "A UDP port (8000), a multicast group (239.1.2.3:8000), interface and group \
                     (192.168.1.5:239.1.2.3:8000), a sender too (10.0.0.9:192.168.1.5:239.1.2.3:8000), or a recording \
                     (.pcap, .rsM, …)",
                )
                .changed()
            {
                set(t, "input", input.as_str());
                changed = true;
            }
            if ui.small_button("…").on_hover_text("Choose an MDI recording").clicked()
                && let Some(p) = pick_file(ctx.base_dir, "MDI recordings", super::source::MDI_EXTENSIONS)
            {
                set(t, "input", p.as_str());
                changed = true;
            }
        });
        ui.end_row();
        row_label(ui, "Reserve");
        let mut n = get_int(t, "buffer_frames").unwrap_or(3);
        if ui
            .add(egui::DragValue::new(&mut n).range(1..=50).suffix(" frames"))
            .on_hover_text("With a sound card: frames (400 ms each) held before transmitting, a reserve against network jitter")
            .changed()
        {
            set(t, "buffer_frames", n);
            changed = true;
        }
        ui.end_row();
    });
    ui.label(
        RichText::new("The channel, the services, the clock and the alternative frequencies come from the MDI.")
            .weak()
            .small(),
    );
    changed
}

fn clock(ui: &mut Ui, doc: &mut DocumentMut) -> bool {
    let mut changed = false;
    let t = section(doc, "time");
    grid(ui, "tx_form_time", |ui| {
        row_label(ui, "Time and date");
        let mut on = get_bool(t, "enabled").unwrap_or(true);
        if ui.checkbox(&mut on, "Send (SDC type 8, once per minute)").changed() {
            set(t, "enabled", on);
            changed = true;
        }
        ui.end_row();
        row_label(ui, "Local time offset");
        let mut has = t.contains_key("local_offset_minutes");
        ui.horizontal(|ui| {
            if ui.checkbox(&mut has, "Signal").changed() {
                if has {
                    set(t, "local_offset_minutes", 0i64);
                } else {
                    remove(t, "local_offset_minutes");
                }
                changed = true;
            }
            if has {
                let mut m = get_int(t, "local_offset_minutes").unwrap_or(0);
                if ui.add(egui::DragValue::new(&mut m).range(-720..=840).speed(30.0).suffix(" min")).changed() {
                    set(t, "local_offset_minutes", (m / 30) * 30);
                    changed = true;
                }
            }
        });
        ui.end_row();
    });
    changed
}

/// The multiplex as a bar: one segment per stream, sized by its bytes.
pub fn capacity_bar(ui: &mut Ui, bar: &PlanBar) {
    let used: usize = bar.segments.iter().map(|s| s.bytes).sum();
    // Per transmission frame, whose duration is the mode's (400 ms for A-D, 100 ms for E).
    let ms = f64::from(bar.frame_ms.max(1));
    let kbps = |bytes: usize| bytes as f64 * 8.0 / ms;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Multiplex").strong());
        ui.label(RichText::new(format!("{:.2} kbit/s MSC capacity, {:.0} % used", kbps(bar.capacity), 100.0 * used as f64 / bar.capacity.max(1) as f64)).weak());
    });
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let (background, audio_fill) = (ui.visuals().extreme_bg_color, ui.visuals().selection.bg_fill);
    painter.rect_filled(rect, egui::CornerRadius::same(4), background);
    let data_fill = Color32::from_rgb(0x1D, 0x9E, 0x75);
    let mut x = rect.left();
    for s in &bar.segments {
        let w = rect.width() * s.bytes as f32 / bar.capacity.max(1) as f32;
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w.max(1.0), rect.height()));
        painter.rect_filled(seg.shrink2(egui::vec2(0.5, 0.0)), egui::CornerRadius::same(3), if s.audio { audio_fill } else { data_fill });
        if s.part_a {
            painter.rect_stroke(seg.shrink(1.0), egui::CornerRadius::same(3), egui::Stroke::new(2.0, PART_A_COLOR), egui::StrokeKind::Inside);
        }
        let text = format!("{}{} · {:.1} kbit/s", s.label, part_a_note(s), kbps(s.bytes));
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
        if galley.size().x + 8.0 < w {
            painter.galley(egui::pos2(seg.left() + 4.0, seg.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        }
        x += w;
    }
    let mut legend = String::new();
    for s in &bar.segments {
        legend.push_str(&format!("{}{}: {:.2} kbit/s   ", s.label, part_a_note(s), kbps(s.bytes)));
    }
    ui.label(RichText::new(legend.trim_end()).weak().small());
}

/// Outline of the part A streams in the bar.
const PART_A_COLOR: Color32 = Color32::from_rgb(0xEF, 0x9F, 0x27);

fn part_a_note(s: &Segment) -> &'static str {
    if s.part_a { " (part A)" } else { "" }
}

/// Ask for a file; the path relative to `base` when it lies below it.
fn pick_file(base: &Path, what: &str, extensions: &[&str]) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_directory(base);
    if !extensions.contains(&"*") {
        dialog = dialog.add_filter(what, extensions);
    }
    dialog.add_filter("All files", &["*"]).pick_file().map(|p| relative(base, &p))
}

fn pick_folder(base: &Path) -> Option<String> {
    rfd::FileDialog::new().set_directory(base).pick_folder().map(|p| relative(base, &p))
}

fn relative(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).map_or_else(|_| p.display().to_string(), |r| r.display().to_string().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_e_is_recognised_in_the_form() {
        // The form's own mode parser must know E, or a mode E document reads as invalid.
        let doc: DocumentMut = "[channel]\nmode = \"E\"\noccupancy = 0\n".parse().unwrap();
        let l = channel_layout(&doc).expect("mode E with occupancy 0 is a valid layout");
        assert_eq!((l.mode, l.occupancy), (RobustnessMode::E, SpectrumOccupancy::SO_0));

        // `canon` strips "-", "_" and spaces but NOT "+", so "DRM+" canonises to "drm+".
        assert_eq!(canon("E"), "e");
        assert_eq!(canon("DRM+"), "drm+");
    }

    #[test]
    fn mode_e_is_offered_only_when_the_rate_allows_it() {
        // The rule the mode selector applies: mode E is filtered out below 100 kHz.
        let offered = |khz: i64| -> Vec<&'static str> {
            MODES
                .iter()
                .copied()
                .filter(|(v, _)| *v != "E" || khz * 1000 >= 100_000)
                .map(|(v, _)| v)
                .collect()
        };
        for khz in [48, 96] {
            assert!(!offered(khz).contains(&"E"), "{khz} kHz must not offer mode E");
        }
        for khz in [120, 192] {
            assert!(offered(khz).contains(&"E"), "{khz} kHz must offer mode E");
        }
        // Every mode is always offered, rate or not.
        assert!(offered(48).contains(&"A") && offered(48).contains(&"D"));

        // The rate selector lists exactly the four rates decdrm-core supports.
        let rates: Vec<&str> = RATES.iter().map(|(v, _)| *v).collect();
        assert_eq!(rates, ["48", "96", "120", "192"]);
        for (v, _) in RATES {
            let hz = v.parse::<u32>().unwrap() * 1000;
            assert!(decdrm_core::params::WorkingRate::new(hz).is_some(), "{v} kHz must be a supported rate");
        }
    }

    /// ES 201 980 §7.5.1: "The MSC may use either 64-QAM or 16-QAM mapping in robustness modes
    /// A, B, C and D and 16-QAM or 4-QAM mapping in robustness mode E."
    #[test]
    fn the_msc_selector_follows_the_robustness_mode() {
        // Mode E's list holds exactly what it defines: both its constellations, and none of
        // the 64-QAM family (which DRM+ does not have).
        assert_eq!(msc_modes_for("E").len(), 2, "mode E's two constellations: {:?}", msc_modes_for("E"));
        assert_eq!(msc_modes_for("E")[0].0, "16-QAM");
        assert_eq!(msc_modes_for("E")[1].0, "4-QAM");
        assert!(msc_mode_allowed("E", "16-QAM"));
        assert!(msc_mode_allowed("E", "4-QAM"), "mode E defines 4-QAM (§7.5.1)");
        for msc in ["64-QAM", "HMsym", "HMmix"] {
            assert!(!msc_mode_allowed("E", msc), "mode E must not offer {msc}");
        }
        // Modes A-D: 16-QAM and 64-QAM with its hierarchical variants; 4-QAM is not DRM30's.
        for msc in ["16-QAM", "64-QAM", "HMsym", "HMmix"] {
            assert!(msc_mode_allowed("A", msc), "mode A must offer {msc}");
            assert!(msc_mode_allowed("B", msc));
        }
        assert!(!msc_mode_allowed("A", "4-QAM"));
        // A-D keep the full list, so their selector is a real choice.
        assert_eq!(msc_modes_for("A").len(), MSC_MODES.len());
        // A document holding a constellation the mode cannot express falls back to the
        // mode's default: 16-QAM for mode E, 64-QAM for the DRM30 modes.
        assert_eq!(msc_mode_for("E", "64-QAM"), "16-QAM");
        assert_eq!(msc_mode_for("E", "HMsym"), "16-QAM");
        assert_eq!(msc_mode_for("E", "4-QAM"), "4-QAM", "an allowed choice is kept");
        assert_eq!(msc_mode_for("B", "16-QAM"), "16-QAM", "an allowed choice is kept");
        assert_eq!(msc_mode_for("B", "HMmix"), "HMmix", "an allowed choice is kept");
        // Canonical spellings come back, so writing one does not rewrite the file forever.
        assert_eq!(msc_mode_for("B", "qam64"), "64-QAM");
    }

    /// §7.5.2: "The SDC may use either 16-QAM or 4-QAM mapping with code rate 0,5 for
    /// robustness modes A, B, C and D and 4-QAM mapping with code rate 0,5 or 0,25 for
    /// robustness mode E."
    #[test]
    fn the_sdc_selector_follows_the_robustness_mode() {
        // Mode E defines 4-QAM only, so its row is a label (one option is not a choice).
        assert_eq!(sdc_modes_for("E").len(), 1, "mode E's SDC row is a label, not a selector");
        assert_eq!(sdc_modes_for("E")[0].0, "4-QAM");
        assert!(sdc_mode_allowed("E", "4-QAM"));
        assert!(!sdc_mode_allowed("E", "16-QAM"), "DRM+ has no 16-QAM SDC");
        // Modes A-D keep both, so their row stays a selector.
        assert_eq!(sdc_modes_for("B").len(), 2);
        assert!(sdc_mode_allowed("B", "16-QAM") && sdc_mode_allowed("B", "4-QAM"));
        assert_eq!(sdc_mode_for("E", "16-QAM"), "4-QAM");
        assert_eq!(sdc_mode_for("E", "4-QAM"), "4-QAM");
        assert_eq!(sdc_mode_for("B", "4-QAM"), "4-QAM", "an allowed choice is kept");
    }

    /// §7.6: "For robustness mode E only one interleaver depth is available which corresponds
    /// to the algorithm for high interleaver depth."
    #[test]
    fn mode_e_has_one_interleaving_depth() {
        assert_eq!(interleaving_for("E", "short"), "long", "DRM+ only has the long depth");
        assert_eq!(interleaving_for("E", "long"), "long");
        assert_eq!(interleaving_for("B", "short"), "short", "DRM30 keeps both");
        assert_eq!(interleaving_for("B", "long"), "long");
        // Mode E's interleaver flag is fixed at 0, so short must never be written.
        let mut doc: DocumentMut = "[channel]\nmode = \"E\"\ninterleaving = \"short\"\n".parse().unwrap();
        repair_channel_for_mode(&mut doc);
        assert_eq!(get_str(section(&mut doc, "channel"), "interleaving").as_deref(), Some("long"));
    }

    /// Switching to mode E must repair a DRM30 document, or the station refuses it: 64-QAM
    /// does not exist in DRM+, nor a 16-QAM SDC, nor short interleaving, nor a hierarchical
    /// layer (§7.5.1: "a hierarchical mapping scheme can only be used in a 64-QAM signal
    /// constellation").
    #[test]
    fn switching_to_mode_e_repairs_the_document() {
        let mut doc: DocumentMut = r#"
[channel]
mode = "E"
occupancy = 0
msc_mode = "HMsym"
sdc_mode = "16-QAM"
interleaving = "short"
protection_b = 3
[[service]]
label = "R"
id = 0xD0D001
[service.audio]
codec = "aac"
input = { tone_hz = 1000.0 }
hierarchical = true
"#
        .parse()
        .unwrap();
        assert!(repair_channel_for_mode(&mut doc));
        let ch = section(&mut doc, "channel");
        assert_eq!(get_str(ch, "msc_mode").as_deref(), Some("16-QAM"), "64-QAM does not exist in DRM+");
        assert_eq!(get_str(ch, "sdc_mode").as_deref(), Some("4-QAM"), "DRM+ has 4-QAM SDC only");
        assert_eq!(get_str(ch, "interleaving").as_deref(), Some("long"), "DRM+ has one depth");
        // Level 3 is a real choice in mode E's own 16-QAM table (§7.5.1 table 31), so the
        // document's level stands rather than being clamped to the DRM30 two.
        assert_eq!(get_int(section(&mut doc, "channel"), "protection_b"), Some(3));
        // The hierarchical mark cannot survive without a hierarchical constellation.
        let text = doc.to_string();
        assert!(!text.contains("hierarchical"), "{text}");
        // The repaired document must be one the station accepts.
        let cfg = decdrm_station::StationConfig::from_toml_str(&text)
            .unwrap_or_else(|e| panic!("repaired mode E document must validate: {e}"));
        assert_eq!(cfg.channel.mode.0, RobustnessMode::E);
        // Repairing twice changes nothing more.
        assert!(!repair_channel_for_mode(&mut doc));
    }

    /// The mode E rules must not touch a document already in order.
    #[test]
    fn a_valid_mode_e_document_is_left_alone() {
        let mut doc: DocumentMut = "[channel]\nmode = \"E\"\noccupancy = 0\nmsc_mode = \"16-QAM\"\nsdc_mode = \"4-QAM\"\ninterleaving = \"long\"\n"
            .parse()
            .unwrap();
        assert!(!repair_channel_for_mode(&mut doc), "nothing to repair");
        // A DRM30 document keeps its own choices: 64-QAM, 16-QAM SDC, short interleaving.
        let mut b: DocumentMut = "[channel]\nmode = \"B\"\nmsc_mode = \"64-QAM\"\nsdc_mode = \"16-QAM\"\ninterleaving = \"short\"\n"
            .parse()
            .unwrap();
        assert!(!repair_channel_for_mode(&mut b));
        assert_eq!(get_str(section(&mut b, "channel"), "msc_mode").as_deref(), Some("64-QAM"));
    }

    /// The repair runs on every frame the Channel card is drawn, so a document already in
    /// order must come back byte for byte — comments, key order, absent keys and equivalent
    /// spellings included. Otherwise merely *looking* at the form would dirty the file and
    /// rewrite what the user wrote.
    #[test]
    fn a_valid_document_keeps_its_text_exactly() {
        // Mode B with no constellation keys at all (their defaults are valid there), a
        // comment on the mode line and a hand-written spelling of the mode.
        let text = "[channel]\nmode = \"b\"   # the usual choice\noccupancy = 3\n\n[[service]]\nlabel = \"R\"\nid = 0xD0D001\n[service.audio]\ncodec = \"aac\"\ninput = { tone_hz = 1000.0 }\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        assert!(!repair_channel_for_mode(&mut doc), "a valid mode B document needs no repair");
        assert_eq!(doc.to_string(), text, "the text must not change");

        // Mode E already in order, with equivalent spellings and its own layout.
        let text = "[channel]\nmode = \"E\"\noccupancy = 0\nmsc_mode = \"16qam\"\nsdc_mode = \"4qam\"\ninterleaving = \"long\"\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        assert!(!repair_channel_for_mode(&mut doc), "an equivalent spelling is already correct");
        assert_eq!(doc.to_string(), text, "the text must not change");
    }

    /// A protection level the station does not use is not rewritten: the station itself only
    /// checks `protection_a` when a stream is in part A and `protection_hierarchical` when the
    /// constellation is hierarchical, so a stale value there is harmless. Changing it would
    /// dirty a document the station accepts, for nothing.
    #[test]
    fn unused_protection_levels_are_left_alone() {
        // 16-QAM has levels 0-1, so protection_b = 3 is out of range and must be clamped; the
        // part A and hierarchical levels are unused here and keep their values.
        let text = "[channel]\nmode = \"B\"\nmsc_mode = \"16-QAM\"\nprotection_a = 3\nprotection_b = 3\nprotection_hierarchical = 3\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        assert!(repair_channel_for_mode(&mut doc));
        let ch = section(&mut doc, "channel");
        assert_eq!(get_int(ch, "protection_b"), Some(1), "the used level is clamped");
        assert_eq!(get_int(ch, "protection_a"), Some(3), "part A is unused: left alone");
        assert_eq!(get_int(ch, "protection_hierarchical"), Some(3), "no hierarchical layer: left alone");
    }

    /// The protection levels a selector offers follow the constellation (§7.5.1 tables 29-32).
    #[test]
    fn protection_levels_follow_the_constellation() {
        assert_eq!(protection_levels("A", "16-QAM").len(), 2, "16-QAM has levels 0-1 in DRM30");
        assert_eq!(protection_levels("A", "64-QAM").len(), 4);
        // Mode E has four of both its constellations (tables 31 and 29), and their code
        // rates are not the DRM30 ones, so the labels differ too.
        assert_eq!(protection_levels("E", "16-QAM").len(), 4, "mode E's 16-QAM has levels 0-3");
        assert_eq!(protection_levels("E", "4-QAM").len(), 4, "mode E's 4-QAM has levels 0-3");
        assert_ne!(
            protection_levels("E", "16-QAM")[0].1,
            protection_levels("A", "16-QAM")[0].1,
            "the same level number is a different code rate in mode E"
        );
    }

    #[test]
    fn mode_e_has_a_fixed_bandwidth_and_cd_are_restricted() {
        // Mode E's row shows a label (occupancy is pinned to 0), so it never uses the selector.
        // C and D are restricted to 10 and 20 kHz; A and B keep all six.
        let allowed = |mode: &str| -> Vec<&str> {
            OCCUPANCIES
                .iter()
                .copied()
                .filter(|(v, _)| !matches!(mode, "C" | "D") || matches!(*v, "3" | "5"))
                .map(|(v, _)| v)
                .collect()
        };
        assert_eq!(allowed("C"), ["3", "5"]);
        assert_eq!(allowed("D"), ["3", "5"]);
        assert_eq!(allowed("A").len(), 6);
        assert_eq!(allowed("B").len(), 6);
        // Mode E is defined at occupancy 0 only, which is what its row pins.
        let (kmin, kmax) = decdrm_core::params::carrier_range(
            RobustnessMode::E, decdrm_core::params::SpectrumOccupancy::SO_0).unwrap();
        assert_eq!((kmin, kmax), (-106, 106));
        assert!(decdrm_core::params::carrier_range(
            RobustnessMode::E, decdrm_core::params::SpectrumOccupancy::SO_3).is_none());
    }

    /// A VHF preset replaces the annex B channel number with the profile plus its speed and
    /// band, and choosing an HF model again removes them: `[simulate]` is
    /// `deny_unknown_fields`, so a key left behind is either a load error or a stale lie.
    #[test]
    fn the_vhf_presets_replace_the_hf_channel_number() {
        let mut doc = DocumentMut::new();
        let sim = section(&mut doc, "simulate");
        set(sim, "channel", 3i64);
        set_simulation_model(sim, "tu6");
        assert_eq!(get_str(sim, "vhf").as_deref(), Some("tu6"));
        assert!(sim.get("channel").is_none(), "the HF number is removed");
        assert_eq!(get_num(sim, "speed_kmh"), Some(0.0), "parked by default");
        assert_eq!(get_num(sim, "carrier_mhz"), Some(200.0), "Band III by default");

        // The speed and band are kept when switching between VHF profiles; only a missing
        // one gets a default.
        set(sim, "speed_kmh", 100.0);
        set(sim, "carrier_mhz", 98.0);
        set_simulation_model(sim, "ra");
        assert_eq!(get_str(sim, "vhf").as_deref(), Some("ra"));
        assert_eq!(get_num(sim, "speed_kmh"), Some(100.0));
        assert_eq!(get_num(sim, "carrier_mhz"), Some(98.0));

        set_simulation_model(sim, "4");
        assert_eq!(get_int(sim, "channel"), Some(4));
        for key in ["vhf", "speed_kmh", "carrier_mhz"] {
            assert!(sim.get(key).is_none(), "{key} left behind");
        }

        // What the form writes is what the station parses.
        let mut doc = DocumentMut::new();
        let sim = section(&mut doc, "simulate");
        set_simulation_model(sim, "tu6");
        set(sim, "speed_kmh", 100.0);
        let text = doc.to_string();
        let cfg = decdrm_station::StationConfig::from_toml_str(&text)
            .unwrap_or_else(|e| panic!("the form's VHF section must parse: {e}\n{text}"));
        let sim = cfg.simulate.expect("the section survives");
        assert_eq!(sim.vhf, Some(decdrm_station::VhfProfile::Tu6));
        assert_eq!(sim.speed_kmh, 100.0);
    }

    #[test]
    fn names_match_like_the_station_does() {        assert_eq!(canon("64-QAM"), canon("QAM64"));
        assert_eq!(canon("64qam"), "64qam");
        assert_eq!(canon("HE-AAC v2"), canon("he_aac_v2"));
        assert_ne!(canon("16-QAM"), canon("64-QAM"));
    }

    /// The sample rate is written as a **string** and must survive a round trip through the
    /// station parser. Writing an integer (which this form did) produced a file the parser then
    /// rejected, so the value silently did not apply.
    #[test]
    fn the_sample_rate_is_written_as_the_station_parses_it() {
        use decdrm_station::config::RateSetting;
        for khz in [48i64, 96, 120, 192] {
            // Read: every spelling the station writes or accepts must come back as kHz.
            for text in [format!("{khz}kHz"), format!("{khz}"), format!("{}", khz * 1000)] {
                let mut doc: DocumentMut = "[channel]
".parse().unwrap();
                set(section(&mut doc, "channel"), "working_sample_rate", text.as_str());
                let mut doc = doc;
                let ch = section(&mut doc, "channel");
                assert_eq!(sample_rate_khz(ch), khz, "reading {text:?}");
            }
            // Write: what the form writes must parse back through the station's own parser.
            let mut doc: DocumentMut = "[channel]
".parse().unwrap();
            set(section(&mut doc, "channel"), "working_sample_rate", format!("{khz}kHz"));
            let written = doc.to_string();
            let cfg = decdrm_station::StationConfig::from_toml_str(&written)
                .unwrap_or_else(|e| panic!("the form's output must parse for {khz} kHz: {e}"));
            assert_eq!(cfg.channel.working_sample_rate.0.hz(), (khz * 1000) as u32);
            assert_eq!(format!("{khz}kHz").parse::<RateSetting>().unwrap().0.hz(), (khz * 1000) as u32);
        }
    }

    /// The bit rate must use the mode's own frame duration. Mode E's frame is 100 ms, so a
    /// constant 400 reported a quarter of the real rate: a mode E station's 2488 bytes per frame
    /// came out as ~50 kbit/s instead of ~199.
    #[test]
    fn the_multiplex_bit_rate_uses_the_modes_frame_duration() {
        let seg = |bytes: usize| Segment {
            label: "x".into(),
            bytes,
            audio: true,
            part_a: false,
            service: Some(0),
        };
        let e = PlanBar { capacity: 2488, frame_ms: 100, segments: vec![seg(2488)] };
        assert!((e.audio_kbps(0).unwrap() - 199.04).abs() < 0.1, "{:?}", e.audio_kbps(0));
        let b = PlanBar { capacity: 1228, frame_ms: 400, segments: vec![seg(1228)] };
        assert!((b.audio_kbps(0).unwrap() - 24.56).abs() < 0.1, "{:?}", b.audio_kbps(0));
        // The duration comes from the mode.
        assert_eq!(RobustnessMode::E.frame_ms(), 100);
        assert_eq!(RobustnessMode::B.frame_ms(), 400);
    }

    #[test]
    fn edits_keep_comments_and_other_content() {
        let mut doc: DocumentMut = "[channel]\nmode = \"B\"   # robustness mode\n\n[[afs.multiplex]]\nkhz = [5990]\n".parse().unwrap();
        set(section(&mut doc, "channel"), "mode", "A");
        let text = doc.to_string();
        assert!(text.contains("mode = \"A\"   # robustness mode"), "{text}");
        assert!(text.contains("[[afs.multiplex]]"), "{text}");
    }

    #[test]
    fn hex_ids_and_inline_inputs() {
        let mut doc: DocumentMut = "[[service]]\nlabel = \"X\"\nid = 0xD0D001\n[service.audio]\ncodec = \"aac\"\ninput = { tone_hz = 600.0 }\n".parse().unwrap();
        let svc = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap().get_mut(0).unwrap();
        set_raw(svc, "id", "0xABCDEF");
        let input = child(child(svc, "audio"), "input");
        assert_eq!(InputKind::of(input), InputKind::Tone);
        set(input, "tone_hz", 1000.0);
        let text = doc.to_string();
        assert!(text.contains("id = 0xABCDEF"), "{text}");
        assert!(text.contains("input = { tone_hz = 1000.0 }"), "{text}");
        // The edited document is still a valid station configuration.
        let cfg: decdrm_station::StationConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg.services[0].id, 0xABCDEF);
    }

    #[test]
    fn part_a_switches() {
        let mut doc: DocumentMut = r#"
[channel]
msc_mode = "16-QAM"
protection_b = 1   # part B
protection_a = 1
[[service]]
label = "R"
id = 0xD0D001
[service.audio]
codec = "aac"
input = { tone_hz = 1000.0 }
[[service.app]]
type = "journaline"
stream = "data"
[[service.app]]
type = "epg"
stream = "data"
[[service.app]]
type = "slideshow"
part = "A"
hierarchical = true
"#
        .parse()
        .unwrap();
        // A hierarchical stream has no part.
        assert!(!uses_part_a(&doc));
        let svc = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap().get_mut(0).unwrap();
        set_part(child(svc, "audio"), true);
        assert!(uses_part_a(&doc));
        // Part A's level drops below part B's; part B's comment stays.
        keep_part_a_stronger(section(&mut doc, "channel"));
        assert_eq!(get_int(section(&mut doc, "channel"), "protection_a"), Some(0));
        assert!(doc.to_string().contains("protection_b = 1   # part B"));
        // A shared stream switches as a whole, and back.
        sync_shared_parts(&mut doc, "data", true);
        assert_eq!(doc.to_string().matches("part = \"A\"").count(), 4, "{doc}");
        sync_shared_parts(&mut doc, "data", false);
        assert_eq!(doc.to_string().matches("part = \"A\"").count(), 2, "{doc}");
        let cfg: decdrm_station::StationConfig = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(cfg.services[0].audio.as_ref().unwrap().part, decdrm_station::Part::A);
        assert_eq!(cfg.services[0].apps[0].part, decdrm_station::Part::B);
        // Nothing is stronger than part B's level 0: part A's level stays (the check
        // then explains); a level that is already stronger stays too.
        let ch = section(&mut doc, "channel");
        set(ch, "protection_b", 0i64);
        keep_part_a_stronger(ch);
        assert_eq!(get_int(ch, "protection_a"), Some(0));
        set(ch, "msc_mode", "64-QAM");
        set(ch, "protection_b", 3i64);
        set(ch, "protection_a", 1i64);
        keep_part_a_stronger(ch);
        assert_eq!(get_int(ch, "protection_a"), Some(1));
    }

    #[test]
    fn audio_shares() {
        // 20 kHz of 64-QAM: room for three HE-AAC services next to the data.
        let text = "[channel]\nmode = \"A\"\noccupancy = 5\nmsc_mode = \"64-QAM\"\n[output]\nfile = \"out.wav\"\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        doc.as_table_mut().insert("service", Item::ArrayOfTables(ArrayOfTables::new()));
        let list = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap();
        list.push(new_service(0, true));
        list.push(new_service(1, false));
        list.push(new_service(2, true));
        // Two audio services, 50/50; a data service has no share.
        let shares = Shares::of(list);
        assert_eq!(shares.weights, [Some(1.0), None, Some(1.0)]);
        assert_eq!((shares.percent(0), shares.percent(1)), (Some(50.0), None));
        assert_eq!(shares.for_new(), None, "no explicit shares: a new service gets the default");
        // 70/30.
        apply_shares(list, &rebalance(&shares.weights, 0, 70.0));
        let shares = Shares::of(list);
        assert_eq!(shares.weights, [Some(70.0), None, Some(30.0)]);
        // A third audio service gets an average share; the others keep theirs.
        assert_eq!(shares.for_new(), Some(50.0));
        let mut svc = new_service(3, true);
        set(child(&mut svc, "audio"), "share", shares.for_new().unwrap());
        list.push(svc);
        // Setting the new one to 20 % keeps 70:30 between the first two.
        let shares = Shares::of(list);
        apply_shares(list, &rebalance(&shares.weights, 3, 20.0));
        assert_eq!(Shares::of(list).weights, [Some(56.0), None, Some(24.0), Some(20.0)]);
        // The station splits the audio capacity (what the data service leaves)
        // accordingly.
        let pages = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/decdrm-station/examples/journaline.toml");
        set(child(list.get_mut(1).unwrap(), "data"), "path", pages);
        let cfg: decdrm_station::StationConfig = toml::from_str(&doc.to_string()).unwrap();
        let plan = cfg.validate().unwrap();
        let (a, b, c) = (plan.service_bitrate(0), plan.service_bitrate(2), plan.service_bitrate(3));
        assert!((a / b - 56.0 / 24.0).abs() < 0.05 && (a / c - 56.0 / 20.0).abs() < 0.05, "{a} {b} {c}");
        // Others with no weight left share the rest equally; a share stays positive.
        assert_eq!(rebalance(&[Some(1.0), Some(0.0), None], 0, 99.95), [Some(100.0), Some(0.1), None]);
        assert_eq!(rebalance(&[Some(2.0), Some(2.0), Some(2.0)], 1, 40.0), [Some(30.0), Some(40.0), Some(30.0)]);
    }

    /// A Journaline application switched to feeds and back; the URLs one per line.
    #[test]
    fn journaline_feeds() {
        let mut doc: DocumentMut = "[[service]]\nlabel = \"D\"\nid = 0xD0D002\n[service.data]\ntype = \"journaline\"\npath = \"pages.toml\"\n"
            .parse()
            .unwrap();
        let svc = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap().get_mut(0).unwrap();
        let app = child(svc, "data");
        // What the source switch does.
        remove(app, "path");
        set(app, "feeds", toml_edit::Array::new());
        let lines = feed_lines(" https://a.example/rss \n\n http://b.example/atom\n");
        set(app, "feeds", lines.iter().map(String::as_str).collect::<toml_edit::Array>());
        set(app, "feed_refresh", 15 * 60);
        assert_eq!(get_feeds(app), ["https://a.example/rss", "http://b.example/atom"]);
        let cfg: decdrm_station::StationConfig = toml::from_str(&doc.to_string()).unwrap();
        let data = cfg.services[0].data.as_ref().unwrap();
        assert_eq!((data.feeds.len(), data.feed_refresh, data.path.as_ref()), (2, Some(900), None));
        let svc = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap().get_mut(0).unwrap();
        let app = child(svc, "data");
        FEED_KEYS.iter().for_each(|k| remove(app, k));
        assert!(!doc.to_string().contains("feed"), "{doc}");
    }

    #[test]
    fn new_services_are_valid() {
        let mut doc: DocumentMut = "[channel]\nmode = \"B\"\noccupancy = 3\nmsc_mode = \"16-QAM\"\n".parse().unwrap();
        doc.as_table_mut().insert("service", Item::ArrayOfTables(ArrayOfTables::new()));
        let list = doc.get_mut("service").and_then(Item::as_array_of_tables_mut).unwrap();
        list.push(new_service(0, true));
        list.push(new_service(1, false));
        let cfg: decdrm_station::StationConfig = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(cfg.services.len(), 2);
        assert_eq!(cfg.services[1].id, 0xD0D002);
    }
}
