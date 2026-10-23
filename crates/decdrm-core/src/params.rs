//! DRM transmission parameters: robustness modes, spectrum occupancy and the OFDM
//! numerology derived from them (ES 201 980 §8.1, tables 47–49).
//!
//! Modes A–D run at [`SAMPLE_RATE`] (48 kHz), at which every one of them has an integer
//! number of samples per useful symbol part and guard interval (the same choice Dream makes).
//! Robustness mode E (DRM+) has 213 carriers reaching ±47.11 kHz, so it needs the higher
//! [`WorkingRate`] — see [`check_mode_rate`].

use std::fmt;

/// Working sample rate of the receiver and transmitter chains, in Hz.
///
/// The default ([`WorkingRate::R48`]). Modes A–D run only at this rate; robustness mode E
/// needs [`MODE_E_MIN_RATE`] or more (see [`WorkingRate`] and [`check_mode_rate`]).
pub const SAMPLE_RATE: u32 = 48_000;

/// The elementary time period T = 83⅓ µs, as its denominator in seconds: T = 1/12000 s
/// (ES 201 980 §8.1).
///
/// Every DRM transmission parameter is a whole number of these periods, and T is a whole
/// number of samples at every [`WorkingRate`] (4, 8, 10 or 16) — which is what keeps the
/// symbol geometry exact instead of accumulating rounding error.
pub const ELEMENTARY_PERIOD_DEN: u32 = 12_000;

/// Lowest working sample rate robustness mode E may use, in Hz.
///
/// Mode E carries 213 carriers (Kmin = −106 … Kmax = +106) at a 444 4/9 Hz spacing, so its
/// highest carrier reaches +47.11 kHz: Nyquist needs more than 94.2 kHz. 100 kHz is the DRM+
/// channel raster (§8.3, table 48) and leaves ~2.9 kHz of margin.
pub const MODE_E_MIN_RATE: u32 = 100_000;

/// The working sample rate of the chains: the rate the OFDM baseband is generated and
/// demodulated at, in Hz.
///
/// Modes A–D run at 48 kHz. Mode E needs [`MODE_E_MIN_RATE`] or more, which is why this is a
/// value the engine carries rather than a single constant.
///
/// Every rate here keeps T a whole number of samples, so the symbol geometry stays exact:
///
/// | rate | samples per T | Tu (27T) | Tg (3T) | Ts (30T) | mode E Nyquist margin |
/// |---|---|---|---|---|---|
/// | 48 kHz | 4 | 108 | 12 | 120 | — (mode E cannot run here) |
/// | 96 kHz | 8 | 216 | 24 | 240 | +0.89 kHz (too tight for real filters) |
/// | 120 kHz | 10 | 270 | 30 | 300 | +12.9 kHz |
/// | 192 kHz | 16 | 432 | 48 | 480 | +48.9 kHz |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkingRate(u32);

impl WorkingRate {
    /// 48 kHz: the rate modes A–D run at.
    pub const R48: Self = Self(48_000);
    /// 96 kHz: the lowest rate that fits mode E's carriers, but with only 0.89 kHz of
    /// Nyquist margin — prefer [`Self::R120`].
    pub const R96: Self = Self(96_000);
    /// 120 kHz: mode E with 12.9 kHz of margin.
    pub const R120: Self = Self(120_000);
    /// 192 kHz: mode E with 48.9 kHz of margin.
    pub const R192: Self = Self(192_000);

    /// The working rates DecDRM accepts, ascending.
    pub const ALL: [Self; 4] = [Self::R48, Self::R96, Self::R120, Self::R192];

    /// A rate of `hz` Hz, if DecDRM supports it.
    pub const fn new(hz: u32) -> Option<Self> {
        match hz {
            48_000 => Some(Self::R48),
            96_000 => Some(Self::R96),
            120_000 => Some(Self::R120),
            192_000 => Some(Self::R192),
            _ => None,
        }
    }

    /// The rate in Hz.
    pub const fn hz(self) -> u32 {
        self.0
    }

    /// Samples per elementary period T (83⅓ µs).
    pub const fn samples_per_t(self) -> usize {
        (self.0 / ELEMENTARY_PERIOD_DEN) as usize
    }

    /// Whether `mode` may be used at this rate (see [`check_mode_rate`]).
    pub fn allows(self, mode: RobustnessMode) -> bool {
        check_mode_rate(mode, self).is_ok()
    }
}

impl Default for WorkingRate {
    fn default() -> Self {
        Self(SAMPLE_RATE)
    }
}

impl From<WorkingRate> for f64 {
    fn from(r: WorkingRate) -> Self {
        f64::from(r.0)
    }
}

impl fmt::Display for WorkingRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} kHz", self.0 as f64 / 1000.0)
    }
}

/// Transmission frames per transmission super frame (§8.1).
pub const FRAMES_PER_SUPERFRAME: usize = 3;

/// Duration of one transmission frame in samples at [`SAMPLE_RATE`] (400 ms).
pub const SAMPLES_PER_FRAME: usize = 19_200;

/// Maximum number of streams in the MSC multiplex (§6.2.1).
pub const MAX_STREAMS: usize = 4;

/// Maximum number of services (§6.3).
pub const MAX_SERVICES: usize = 4;

/// DRM robustness mode (§8.1, table 47; §4.4.2.2, table 1).
///
/// Modes A–D are the DRM30 modes used below 30 MHz. **Mode E is DRM+**: the VHF mode, in
/// band II (the FM band) on a 100 kHz raster. It differs from the others in more than
/// numerology — its frame is 100 ms rather than 400 ms, it leaves no carriers unused, it
/// defines no frequency reference cells, it has AFS reference cells the others lack, its FAC
/// carries two sets of service parameters, and it needs a working sample rate above ~94 kHz
/// (see [`check_mode_rate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RobustnessMode {
    A,
    B,
    C,
    D,
    E,
}

impl RobustnessMode {
    /// The DRM30 modes A–D, in order.
    pub const DRM30: [RobustnessMode; 4] = [Self::A, Self::B, Self::C, Self::D];

    /// Every mode, in order.
    pub const ALL: [RobustnessMode; 5] = [Self::A, Self::B, Self::C, Self::D, Self::E];

    /// Index 0..=4, used to address per-mode tables.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Decode the 2-bit robustness mode field used in SDC/MDI signalling.
    ///
    /// The field cannot express mode E: §6.3 gives the value `1` a second meaning ("the FAC
    /// block contains two sets of service parameters") when the SDC says so, so mode E is
    /// recognised from the SDC rather than from these two bits alone.
    pub const fn from_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(Self::A),
            1 => Some(Self::B),
            2 => Some(Self::C),
            3 => Some(Self::D),
            _ => None,
        }
    }

    /// `true` for robustness mode E (DRM+).
    pub const fn is_drm_plus(self) -> bool {
        matches!(self, Self::E)
    }

    /// Useful symbol part Tu in units of the elementary period T
    /// (§8.1, table 2: 288, 256, 176, 112, 27).
    pub const fn tu_in_t(self) -> usize {
        match self {
            Self::A => 288,
            Self::B => 256,
            Self::C => 176,
            Self::D => 112,
            Self::E => 27,
        }
    }

    /// Guard interval Tg in units of the elementary period T
    /// (§8.1, table 2: 32, 64, 64, 88, 3).
    pub const fn tg_in_t(self) -> usize {
        match self {
            Self::A => 32,
            Self::B => 64,
            Self::C => 64,
            Self::D => 88,
            Self::E => 3,
        }
    }

    /// Length of the useful symbol part Tu in samples at `rate` (= FFT size).
    ///
    /// Exact for every [`WorkingRate`], because T is a whole number of samples there.
    pub const fn fft_size_at(self, rate: WorkingRate) -> usize {
        self.tu_in_t() * rate.samples_per_t()
    }

    /// Ratio Tg/Tu as (numerator, denominator) (§8.1, table 2).
    pub const fn guard_ratio(self) -> (usize, usize) {
        match self {
            Self::A => (1, 9),
            Self::B => (1, 4),
            Self::C => (4, 11),
            Self::D => (11, 14),
            Self::E => (1, 9),
        }
    }

    /// Guard interval length Tg in samples at `rate`.
    pub const fn guard_len_at(self, rate: WorkingRate) -> usize {
        self.tg_in_t() * rate.samples_per_t()
    }

    /// Total symbol length Ts = Tu + Tg in samples at `rate`.
    pub const fn symbol_len_at(self, rate: WorkingRate) -> usize {
        self.fft_size_at(rate) + self.guard_len_at(rate)
    }

    /// Length of the useful symbol part Tu in samples at the default 48 kHz (§8.1, table 2).
    ///
    /// A–D only: mode E has no geometry at this rate (see [`check_mode_rate`]).
    pub const fn fft_size(self) -> usize {
        self.fft_size_at(WorkingRate::R48)
    }

    /// Guard interval length Tg in samples at the default 48 kHz.
    pub const fn guard_len(self) -> usize {
        self.guard_len_at(WorkingRate::R48)
    }

    /// Total symbol length Ts = Tu + Tg in samples at the default 48 kHz.
    pub const fn symbol_len(self) -> usize {
        self.symbol_len_at(WorkingRate::R48)
    }

    /// Number of OFDM symbols per transmission frame Ns (§8.1, table 47).
    pub const fn symbols_per_frame(self) -> usize {
        match self {
            Self::A | Self::B => 15,
            Self::C => 20,
            Self::D => 24,
            Self::E => 40,
        }
    }

    /// Transmission frames per transmission super frame (§8.1): three for A–D, four for E.
    pub const fn frames_per_superframe(self) -> usize {
        match self {
            Self::A | Self::B | Self::C | Self::D => 3,
            // "each transmission super frame consists of four transmission frames" (§8.1).
            Self::E => 4,
        }
    }

    /// Duration of one transmission frame, ms: 400 for A–D, **100 for mode E** (§8.1, table 2).
    pub const fn frame_ms(self) -> u32 {
        match self {
            Self::A | Self::B | Self::C | Self::D => 400,
            Self::E => 100,
        }
    }

    /// Samples in one transmission frame at `rate`.
    pub const fn samples_per_frame_at(self, rate: WorkingRate) -> usize {
        self.symbol_len_at(rate) * self.symbols_per_frame()
    }

    /// Number of OFDM symbols per transmission super frame.
    pub const fn symbols_per_superframe(self) -> usize {
        self.symbols_per_frame() * self.frames_per_superframe()
    }

    /// Carrier spacing 1/Tu in Hz at `rate`.
    pub fn carrier_spacing_at(self, rate: WorkingRate) -> f64 {
        f64::from(rate.hz()) / self.fft_size_at(rate) as f64
    }

    /// Carrier spacing 1/Tu in Hz at the default 48 kHz.
    pub fn carrier_spacing(self) -> f64 {
        self.carrier_spacing_at(WorkingRate::R48)
    }

    /// Number of OFDM symbols at the start of each super frame that carry the SDC.
    pub const fn sdc_symbols(self) -> usize {
        match self {
            Self::A | Self::B => 2,
            Self::C | Self::D => 3,
            // §8.5.3.1: mode E's SDC occupies symbols 0..=4 of each super frame.
            Self::E => 5,
        }
    }
}

impl fmt::Display for RobustnessMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = match self {
            Self::A => 'A',
            Self::B => 'B',
            Self::C => 'C',
            Self::D => 'D',
            Self::E => 'E',
        };
        write!(f, "{c}")
    }
}

/// Spectrum occupancy 0..=5 (§8.1, table 83): nominal channel bandwidths of
/// 4.5, 5, 9, 10, 18 and 20 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpectrumOccupancy(u8);

impl SpectrumOccupancy {
    pub const SO_0: Self = Self(0);
    pub const SO_1: Self = Self(1);
    pub const SO_2: Self = Self(2);
    pub const SO_3: Self = Self(3);
    pub const SO_4: Self = Self(4);
    pub const SO_5: Self = Self(5);

    pub const ALL: [SpectrumOccupancy; 6] =
        [Self::SO_0, Self::SO_1, Self::SO_2, Self::SO_3, Self::SO_4, Self::SO_5];

    /// Values 0..=5 are valid; the FAC field is 3 bits so 6 and 7 are rejected.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 5 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Nominal channel bandwidth in kHz.
    pub const fn bandwidth_khz(self) -> f64 {
        match self.0 {
            0 => 4.5,
            1 => 5.0,
            2 => 9.0,
            3 => 10.0,
            4 => 18.0,
            _ => 20.0,
        }
    }
}

impl fmt::Display for SpectrumOccupancy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SO{} ({} kHz)", self.0, self.bandwidth_khz())
    }
}

/// Lowest and highest carrier index (Kmin, Kmax) for a mode/occupancy pair
/// (§8.3, table 49). Returns `None` for combinations the standard does not define.
///
/// Modes C and D exist only at occupancies 3 and 5; **mode E at occupancy 0 only** — the
/// 100 kHz DRM+ raster — with Kmin = −106, Kmax = +106 (213 carriers).
pub const fn carrier_range(mode: RobustnessMode, so: SpectrumOccupancy) -> Option<(i32, i32)> {
    const KMIN: [[i32; 4]; 6] = [
        [2, 1, 0, 0],
        [2, 1, 0, 0],
        [-102, -91, 0, 0],
        [-114, -103, -69, -44],
        [-98, -87, 0, 0],
        [-110, -99, -67, -43],
    ];
    const KMAX: [[i32; 4]; 6] = [
        [102, 91, 0, 0],
        [114, 103, 0, 0],
        [102, 91, 0, 0],
        [114, 103, 69, 44],
        [314, 279, 0, 0],
        [350, 311, 213, 135],
    ];
    match mode {
        // Mode E is defined at spectrum occupancy 0 only (`E as usize` is 4, past the
        // per-occupancy tables, which is why it is handled before indexing them).
        RobustnessMode::E => {
            if so.index() == 0 { Some((-106, 106)) } else { None }
        }
        _ => {
            let kmin = KMIN[so.index()][mode.index()];
            let kmax = KMAX[so.index()][mode.index()];
            if kmin == 0 && kmax == 0 { None } else { Some((kmin, kmax)) }
        }
    }
}

/// Carriers around DC that are unused (§8.3.1, table 50).
///
/// Mode E uses **every** carrier in `Kmin..=Kmax`; A–D leave DC unused, and mode A also its
/// two neighbours.
pub const fn unused_carriers(mode: RobustnessMode) -> &'static [i32] {
    match mode {
        RobustnessMode::A => &[-1, 0, 1],
        RobustnessMode::B | RobustnessMode::C | RobustnessMode::D => &[0],
        RobustnessMode::E => &[],
    }
}

/// Why a robustness mode and a working sample rate cannot be used together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateError {
    /// Robustness mode E's carriers extend past the Nyquist frequency of this rate.
    #[error(
        "robustness mode E (DRM+) needs a working sample rate of at least {min_khz} kHz: its \
         carriers reach +47.1 kHz, above the {nyquist_khz} kHz Nyquist limit of {rate} \
         (choose 120 kHz or 192 kHz)"
    )]
    ModeERateTooLow {
        /// The rate that was asked for.
        rate: WorkingRate,
        /// Nyquist frequency of that rate, kHz, for the message.
        nyquist_khz: u32,
        /// The lowest rate the mode accepts, kHz.
        min_khz: u32,
    },
}

/// Check that `mode` can run at `rate`, or say why not.
///
/// Modes A–D are defined at 48 kHz; mode E needs [`MODE_E_MIN_RATE`] or more. Callers turn
/// this into a user-facing error (transmitter config, GUI, CLI) rather than generating a
/// signal that folds its own carriers. A user interface can use it to hide mode E from its
/// mode selector while the chosen rate is too low.
pub const fn check_mode_rate(mode: RobustnessMode, rate: WorkingRate) -> Result<(), RateError> {
    match mode {
        RobustnessMode::A | RobustnessMode::B | RobustnessMode::C | RobustnessMode::D => Ok(()),
        RobustnessMode::E => {
            if rate.hz() >= MODE_E_MIN_RATE {
                Ok(())
            } else {
                Err(RateError::ModeERateTooLow {
                    rate,
                    nyquist_khz: rate.hz() / 2000,
                    min_khz: MODE_E_MIN_RATE / 1000,
                })
            }
        }
    }
}

/// A valid (mode, occupancy) combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelLayout {
    pub mode: RobustnessMode,
    pub occupancy: SpectrumOccupancy,
}

impl ChannelLayout {
    pub fn new(mode: RobustnessMode, occupancy: SpectrumOccupancy) -> Option<Self> {
        carrier_range(mode, occupancy).map(|_| Self { mode, occupancy })
    }

    /// (Kmin, Kmax) — always defined for a constructed layout.
    pub fn carrier_range(self) -> (i32, i32) {
        carrier_range(self.mode, self.occupancy).expect("validated in ChannelLayout::new")
    }

    /// Number of carriers from Kmin to Kmax inclusive (including unused DC carriers).
    pub fn num_carriers(self) -> usize {
        let (kmin, kmax) = self.carrier_range();
        (kmax - kmin + 1) as usize
    }
}

impl fmt::Display for ChannelLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mode {} / {}", self.mode, self.occupancy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_fills_exactly_400_ms() {
        // DRM30 frames are 400 ms; mode E's is 100 ms.
        for m in RobustnessMode::DRM30 {
            assert_eq!(m.symbol_len() * m.symbols_per_frame(), SAMPLES_PER_FRAME, "mode {m}");
        }
        let e = RobustnessMode::E;
        assert_eq!(e.symbol_len() * e.symbols_per_frame(), 4_800, "mode E is 100 ms @48k");
    }

    #[test]
    fn guard_lengths_match_spec() {
        let g: Vec<_> = RobustnessMode::DRM30.iter().map(|m| m.guard_len()).collect();
        assert_eq!(g, [128, 256, 256, 352]);
        // Mode E at 192 kHz: Tg = 3T = 48 samples.
        assert_eq!(RobustnessMode::E.guard_len_at(WorkingRate::R192), 48);
    }

    #[test]
    fn every_mode_and_rate_fills_its_frame_exactly() {
        // No rounding anywhere: Ts·Ns must equal frame_ms × rate at every supported rate
        // where the mode may run.
        for m in RobustnessMode::ALL {
            for r in WorkingRate::ALL {
                if check_mode_rate(m, r).is_err() {
                    continue;
                }
                let want = u64::from(m.frame_ms()) * u64::from(r.hz()) / 1000;
                assert_eq!(m.samples_per_frame_at(r) as u64, want, "mode {m} at {r}");
            }
        }
    }

    #[test]
    fn working_rates_keep_the_elementary_period_whole() {
        // T = 83⅓ µs must be a whole number of samples at every rate, or the geometry drifts.
        assert_eq!(WorkingRate::R48.samples_per_t(), 4);
        assert_eq!(WorkingRate::R96.samples_per_t(), 8);
        assert_eq!(WorkingRate::R120.samples_per_t(), 10);
        assert_eq!(WorkingRate::R192.samples_per_t(), 16);
        assert_eq!(WorkingRate::new(48_000), Some(WorkingRate::R48));
        assert_eq!(WorkingRate::new(44_100), None);
        assert_eq!(WorkingRate::default(), WorkingRate::R48);
        assert_eq!(WorkingRate::R192.to_string(), "192 kHz");
    }

    #[test]
    fn mode_e_geometry_matches_the_spec() {
        let e = RobustnessMode::E;
        // Table 47: Tu 2.25 ms, Tg 0.25 ms, Ts 2.5 ms, Ns 40, frame 100 ms, super 400 ms.
        assert_eq!(e.tu_in_t(), 27);
        assert_eq!(e.tg_in_t(), 3);
        assert_eq!(e.guard_ratio(), (1, 9));
        assert_eq!(e.symbols_per_frame(), 40);
        assert_eq!(e.frame_ms(), 100);
        assert_eq!(e.frames_per_superframe(), 4);
        assert_eq!(e.symbols_per_superframe(), 160);
        assert_eq!(e.sdc_symbols(), 5);
        assert!(e.is_drm_plus());
        // At 192 kHz: Tu = 27×16 = 432, Tg = 3×16 = 48, Ts = 480 (verified on real DRM+).
        assert_eq!(e.fft_size_at(WorkingRate::R192), 432);
        assert_eq!(e.guard_len_at(WorkingRate::R192), 48);
        assert_eq!(e.symbol_len_at(WorkingRate::R192), 480);
        assert_eq!(e.fft_size_at(WorkingRate::R120), 270);
        assert_eq!(e.symbol_len_at(WorkingRate::R120), 300);
        // Carrier spacing is 1/Tu = 4000/9 Hz at every rate.
        for r in WorkingRate::ALL {
            assert!((e.carrier_spacing_at(r) - 4000.0 / 9.0).abs() < 1e-9, "{r}");
        }
    }

    #[test]
    fn mode_e_has_213_carriers_and_no_unused_ones() {
        let (kmin, kmax) = carrier_range(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
        assert_eq!((kmin, kmax), (-106, 106));
        assert_eq!(kmax - kmin + 1, 213);
        assert_eq!(unused_carriers(RobustnessMode::E), &[] as &[i32]);
        // Its highest carrier is what forces the higher rate: measured at +47.11 kHz, above
        // 24 kHz, so it would fold at 48 kHz.
        let top_hz = f64::from(106) * RobustnessMode::E.carrier_spacing_at(WorkingRate::R192);
        assert!(top_hz > 47_000.0 && top_hz < 47_200.0, "{top_hz}");
        assert!(top_hz > f64::from(SAMPLE_RATE) / 2.0, "would fold at 48 kHz");
        // Mode E exists at occupancy 0 only.
        for so in SpectrumOccupancy::ALL {
            assert_eq!(
                carrier_range(RobustnessMode::E, so).is_some(),
                so.index() == 0,
                "mode E with {so}"
            );
        }
        // A-D still leave their DC carriers unused.
        assert_eq!(unused_carriers(RobustnessMode::A), &[-1, 0, 1]);
        assert_eq!(unused_carriers(RobustnessMode::B), &[0]);
        assert_eq!(unused_carriers(RobustnessMode::D), &[0]);
    }

    #[test]
    fn mode_e_needs_a_high_enough_rate() {
        for m in RobustnessMode::DRM30 {
            assert!(check_mode_rate(m, WorkingRate::R48).is_ok(), "mode {m}");
        }
        assert!(check_mode_rate(RobustnessMode::E, WorkingRate::R48).is_err());
        // 96 kHz technically fits but with only 0.89 kHz of margin: refused by policy.
        assert!(check_mode_rate(RobustnessMode::E, WorkingRate::R96).is_err());
        assert!(check_mode_rate(RobustnessMode::E, WorkingRate::R120).is_ok());
        assert!(check_mode_rate(RobustnessMode::E, WorkingRate::R192).is_ok());
        assert!(!WorkingRate::R48.allows(RobustnessMode::E));
        assert!(!WorkingRate::R96.allows(RobustnessMode::E));
        assert!(WorkingRate::R120.allows(RobustnessMode::E));
        // The message names the rate tried, the limit it missed and what to choose.
        let msg = check_mode_rate(RobustnessMode::E, WorkingRate::R48).unwrap_err().to_string();
        assert!(msg.contains("100 kHz"), "{msg}");
        assert!(msg.contains("47.1 kHz"), "{msg}");
        assert!(msg.contains("48 kHz"), "{msg}");
    }

    #[test]
    fn modes_c_and_d_only_have_so3_and_so5() {
        for so in SpectrumOccupancy::ALL {
            let defined = matches!(so.value(), 3 | 5);
            assert_eq!(ChannelLayout::new(RobustnessMode::C, so).is_some(), defined);
            assert_eq!(ChannelLayout::new(RobustnessMode::D, so).is_some(), defined);
            assert!(ChannelLayout::new(RobustnessMode::A, so).is_some());
            assert!(ChannelLayout::new(RobustnessMode::B, so).is_some());
        }
    }
}
