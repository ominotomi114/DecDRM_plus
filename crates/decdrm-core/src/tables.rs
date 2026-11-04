//! Constant tables from ES 201 980 (transcribed via Dream's `tables/TableCarMap.cpp`,
//! `tables/TableMLC.h` and `tables/TableQAMMapping.h`).
//!
//! Phases are "normalised to 1024" as in the standard: a value `p` means an angle of
//! `2π·p/1024`.

use crate::params::RobustnessMode;

// ---------------------------------------------------------------------------------
// Pilots and FAC positions (§8.4)
// ---------------------------------------------------------------------------------

/// Number of FAC cells per transmission frame (§8.5.2).
///
/// 65 for modes A–D; **244 for robustness mode E** (verified against the spec's stated count
/// and against real DRM+ recordings).
pub const fn num_fac_cells(mode: RobustnessMode) -> usize {
    match mode {
        RobustnessMode::A | RobustnessMode::B | RobustnessMode::C | RobustnessMode::D => 65,
        RobustnessMode::E => 244,
    }
}

/// FAC cell positions (symbol within frame, carrier index) (§8.5.2, tables 62–66).
///
/// The per-mode tables have different lengths, so this returns a slice rather than a fixed
/// array. Mode E has 244 cells spread over symbols 5..=26.
pub const fn fac_positions(mode: RobustnessMode) -> &'static [(u8, i16)] {
    match mode {
        RobustnessMode::A => &FAC_A,
        RobustnessMode::B => &FAC_B,
        RobustnessMode::C => &FAC_C,
        RobustnessMode::D => &FAC_D,
        RobustnessMode::E => &FAC_E,
    }
}

/// Mode E FAC cell positions (§8.5.2, table 66), 244 cells over symbols 5..=26.
///
/// The four carrier groups cycle with **period 4** from symbol 5 (verified against real
/// DRM+): A (11 carriers, remainder 2 mod 16), B (12, rem 6), C (12, rem 10), D (11, rem 14).
/// Built by `fac_positions_e` below from those groups so the cycle cannot be mis-typed.
static FAC_E: [(u8, i16); 244] = build_fac_e();

const FAC_E_GROUPS: [[i16; 12]; 4] = [
    // Group A (11 used; the 12th slot is padding so every row has one type).
    [-78, -62, -46, -30, -14, 2, 18, 34, 50, 66, 82, 0],
    // Group B (12)
    [-90, -74, -58, -42, -26, -10, 6, 22, 38, 54, 70, 86],
    // Group C (12)
    [-86, -70, -54, -38, -22, -6, 10, 26, 42, 58, 74, 90],
    // Group D (11)
    [-82, -66, -50, -34, -18, -2, 14, 30, 46, 62, 78, 0],
];
const FAC_E_GROUP_LEN: [usize; 4] = [11, 12, 12, 11];

/// Build table 66: symbols 5..=25 each carry one full group, cycling A,B,C,D with period 4;
/// symbol 26 carries only the first **3** of group B's carriers, which is what makes the
/// frame's total exactly the 244 cells the standard states.
///
/// (A pure cycle over symbols 5..=26 would give 253 cells, so the truncated last row is real
/// and not an extraction artefact: 5 full cycles = 230, plus group A = 11, plus 3 = 244.)
const fn build_fac_e() -> [(u8, i16); 244] {
    let mut out = [(0u8, 0i16); 244];
    let mut n = 0;
    let mut s = 5;
    while s <= 26 {
        let g = (s - 5) % 4;
        // The last symbol of the frame carries a partial group.
        let len = if s == 26 { 3 } else { FAC_E_GROUP_LEN[g] };
        let mut i = 0;
        while i < len {
            out[n] = (s as u8, FAC_E_GROUPS[g][i]);
            n += 1;
            i += 1;
        }
        s += 1;
    }
    out
}

const FAC_A: [(u8, i16); 65] = [
    (2, 26), (2, 46), (2, 66), (2, 86),
    (3, 10), (3, 30), (3, 50), (3, 70), (3, 90),
    (4, 14), (4, 22), (4, 34), (4, 62), (4, 74), (4, 94),
    (5, 26), (5, 38), (5, 58), (5, 66), (5, 78),
    (6, 22), (6, 30), (6, 42), (6, 62), (6, 70), (6, 82),
    (7, 26), (7, 34), (7, 46), (7, 66), (7, 74), (7, 86),
    (8, 10), (8, 30), (8, 38), (8, 50), (8, 58), (8, 70), (8, 78), (8, 90),
    (9, 14), (9, 22), (9, 34), (9, 42), (9, 62), (9, 74), (9, 82), (9, 94),
    (10, 26), (10, 38), (10, 46), (10, 66), (10, 86),
    (11, 10), (11, 30), (11, 50), (11, 70), (11, 90),
    (12, 14), (12, 34), (12, 74), (12, 94),
    (13, 38), (13, 58), (13, 78),
];

const FAC_B: [(u8, i16); 65] = [
    (2, 13), (2, 25), (2, 43), (2, 55), (2, 67),
    (3, 15), (3, 27), (3, 45), (3, 57), (3, 69),
    (4, 17), (4, 29), (4, 47), (4, 59), (4, 71),
    (5, 19), (5, 31), (5, 49), (5, 61), (5, 73),
    (6, 9), (6, 21), (6, 33), (6, 51), (6, 63), (6, 75),
    (7, 11), (7, 23), (7, 35), (7, 53), (7, 65), (7, 77),
    (8, 13), (8, 25), (8, 37), (8, 55), (8, 67), (8, 79),
    (9, 15), (9, 27), (9, 39), (9, 57), (9, 69), (9, 81),
    (10, 17), (10, 29), (10, 41), (10, 59), (10, 71), (10, 83),
    (11, 19), (11, 31), (11, 43), (11, 61), (11, 73),
    (12, 21), (12, 33), (12, 45), (12, 63), (12, 75),
    (13, 23), (13, 35), (13, 47), (13, 65), (13, 77),
];

const FAC_C: [(u8, i16); 65] = [
    (3, 9), (3, 21), (3, 45), (3, 57),
    (4, 23), (4, 35), (4, 47),
    (5, 13), (5, 25), (5, 37), (5, 49),
    (6, 15), (6, 27), (6, 39), (6, 51),
    (7, 5), (7, 17), (7, 29), (7, 41), (7, 53),
    (8, 7), (8, 19), (8, 31), (8, 43), (8, 55),
    (9, 9), (9, 21), (9, 45), (9, 57),
    (10, 23), (10, 35), (10, 47),
    (11, 13), (11, 25), (11, 37), (11, 49),
    (12, 15), (12, 27), (12, 39), (12, 51),
    (13, 5), (13, 17), (13, 29), (13, 41), (13, 53),
    (14, 7), (14, 19), (14, 31), (14, 43), (14, 55),
    (15, 9), (15, 21), (15, 45), (15, 57),
    (16, 23), (16, 35), (16, 47),
    (17, 13), (17, 25), (17, 37), (17, 49),
    (18, 15), (18, 27), (18, 39), (18, 51),
];

const FAC_D: [(u8, i16); 65] = [
    (3, 9), (3, 18), (3, 27),
    (4, 10), (4, 19),
    (5, 11), (5, 20), (5, 29),
    (6, 12), (6, 30),
    (7, 13), (7, 22), (7, 31),
    (8, 5), (8, 14), (8, 23), (8, 32),
    (9, 6), (9, 15), (9, 24), (9, 33),
    (10, 16), (10, 25), (10, 34),
    (11, 8), (11, 17), (11, 26), (11, 35),
    (12, 9), (12, 18), (12, 27), (12, 36),
    (13, 10), (13, 19), (13, 37),
    (14, 11), (14, 20), (14, 29),
    (15, 12), (15, 30),
    (16, 13), (16, 22), (16, 31),
    (17, 5), (17, 14), (17, 23), (17, 32),
    (18, 6), (18, 15), (18, 24), (18, 33),
    (19, 16), (19, 25), (19, 34),
    (20, 8), (20, 17), (20, 26), (20, 35),
    (21, 9), (21, 18), (21, 27), (21, 36),
    (22, 10), (22, 19), (22, 37),
];

/// Frequency reference pilots: (carrier, phase₁₀₂₄) (§8.4.2, table 51). They are present in
/// every symbol. In mode D the first two pilots are phase-inverted (+512) on odd symbols.
///
/// Empty for robustness mode E: "For robustness mode E, no frequency reference cells are
/// defined" (§8.4.2).
pub const fn freq_pilots(mode: RobustnessMode) -> &'static [(i16, u16)] {
    match mode {
        RobustnessMode::A => &[(18, 205), (54, 836), (72, 215)],
        RobustnessMode::B => &[(16, 331), (48, 651), (64, 555)],
        RobustnessMode::C => &[(11, 214), (33, 392), (44, 242)],
        RobustnessMode::D => &[(7, 788), (21, 1014), (28, 332)],
        RobustnessMode::E => &[],
    }
}

/// Time reference pilots in symbol 0 of each frame: (carrier, phase₁₀₂₄)
/// (§8.4.3.1, tables 53–57).
pub const fn time_pilots(mode: RobustnessMode) -> &'static [(i16, u16)] {
    match mode {
        RobustnessMode::A => &TIME_A,
        RobustnessMode::B => &TIME_B,
        RobustnessMode::C => &TIME_C,
        RobustnessMode::D => &TIME_D,
        RobustnessMode::E => &TIME_E,
    }
}

/// Mode E time reference cells (§8.4.3.1, table 57), 21 cells.
/// "All time reference cells have a power gain of 2" (§8.4.3.2).
const TIME_E: [(i16, u16); 21] = [
    (-80, 219), (-79, 475), (-77, 987), (-53, 652), (-52, 652), (-51, 140),
    (-32, 819), (-31, 819), (12, 907), (13, 907), (14, 651), (21, 903),
    (22, 391), (23, 903), (40, 203), (41, 203), (42, 203), (67, 797),
    (68, 29), (79, 508), (80, 508),
];

const TIME_A: [(i16, u16); 21] = [
    (17, 973), (18, 205), (19, 717), (21, 264), (28, 357), (29, 357), (32, 952),
    (33, 440), (39, 856), (40, 88), (41, 88), (53, 68), (54, 836), (55, 836),
    (56, 836), (60, 1008), (61, 1008), (63, 752), (71, 215), (72, 215), (73, 727),
];
const TIME_B: [(i16, u16); 19] = [
    (14, 304), (16, 331), (18, 108), (20, 620), (24, 192), (26, 704), (32, 44),
    (36, 432), (42, 588), (44, 844), (48, 651), (49, 651), (50, 651), (54, 460),
    (56, 460), (62, 944), (64, 555), (66, 940), (68, 428),
];
const TIME_C: [(i16, u16); 19] = [
    (8, 722), (10, 466), (11, 214), (12, 214), (14, 479), (16, 516), (18, 260),
    (22, 577), (24, 662), (28, 3), (30, 771), (32, 392), (33, 392), (36, 37),
    (38, 37), (42, 474), (44, 242), (45, 242), (46, 754),
];
const TIME_D: [(i16, u16); 21] = [
    (5, 636), (6, 124), (7, 788), (8, 788), (9, 200), (11, 688), (12, 152),
    (14, 920), (15, 920), (17, 644), (18, 388), (20, 652), (21, 1014), (23, 176),
    (24, 176), (26, 752), (27, 496), (28, 332), (29, 432), (30, 964), (32, 452),
];

/// Which arithmetic §8.4.4.3.1 uses to turn (n, m, p) into a gain-reference phase.
///
/// The two differ structurally, not just in constants: modes A–D use a scalar `Q` and a
/// `(1 + s)` factor, while mode E uses three 4×10 matrices and no `s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainPhaseModel {
    /// Modes A–D: `phase = (4·Z₂₅₆[n][m] + p·W₁₀₂₄[n][m] + p²·(1 + s)·Q₁₀₂₄) mod 1024`.
    Wzq256,
    /// Mode E: `phase = (p²·R₁₀₂₄[n][m] + p·Z₁₀₂₄[n][m] + Q₁₀₂₄[n][m]) mod 1024`.
    Rzq1024,
}

/// Parameters of the gain reference (scattered) pilot grid (§8.4.4).
///
/// Verified against real DRM+ recordings for mode E: the grid and the phase formula both
/// reproduce every gain-reference cell (203/203 within 45°, worst residual 22°).
#[derive(Debug, Clone, Copy)]
pub struct ScatteredPilotParams {
    /// Carrier spacing between gain references within one symbol is `freq_int · time_int`,
    /// and the pattern shifts by `freq_int` carriers every symbol.
    pub freq_int: usize,
    /// Period of the pattern in symbols.
    pub time_int: usize,
    /// Constants (x, y, k₀) of the phase formula (§8.4.4.3.1, table 60).
    pub x: i32,
    pub y: i32,
    pub k0: i32,
    /// Which phase arithmetic to use.
    pub model: GainPhaseModel,
    /// Row size of the matrices.
    pub wz_cols: usize,
    /// `W₁₀₂₄[n][m]`, row-major `y × wz_cols` (modes A–D; empty for mode E).
    pub w: &'static [i32],
    /// `Z₂₅₆[n][m]` (modes A–D) or `Z₁₀₂₄[n][m]` (mode E), row-major.
    pub z: &'static [i32],
    /// `Q₁₀₂₄`, a scalar for modes A–D.
    pub q: i32,
    /// `R₁₀₂₄[n][m]`, row-major (mode E only; empty for A–D).
    pub r: &'static [i32],
    /// `Q₁₀₂₄[n][m]`, row-major (mode E only; empty for A–D).
    pub q1024: &'static [i32],
}

/// Mode E gain-reference phase matrices, 4 rows × 10 columns (§8.4.4.3.6), row-major.
///
/// Mode E uses a structurally different formula from A–D (three matrices, no `(1 + s)` term).
/// Verified against real DRM+ recordings: every gain-reference cell phase reproduces.
const GAIN_E_R: [i32; 40] = [
    39, 118, 197, 276, 354, 433, 39, 118, 197, 276,
    37, 183, 402, 37, 183, 402, 37, 183, 402, 37,
    110, 329, 475, 110, 329, 475, 110, 329, 475, 110,
    79, 158, 236, 315, 394, 473, 79, 158, 236, 315,
];
const GAIN_E_Z: [i32; 40] = [
    473, 394, 315, 236, 158, 79, 0, 0, 0, 0,
    183, 914, 402, 37, 475, 841, 768, 768, 987, 183,
    549, 622, 475, 110, 37, 622, 256, 768, 329, 549,
    79, 158, 236, 315, 394, 473, 158, 315, 473, 630,
];
const GAIN_E_Q: [i32; 40] = [
    329, 489, 894, 419, 607, 519, 1020, 942, 817, 939,
    824, 1023, 74, 319, 225, 207, 348, 422, 395, 92,
    959, 379, 7, 738, 500, 920, 440, 727, 263, 733,
    907, 946, 924, 91, 189, 133, 910, 804, 1022, 433,
];

pub const fn scattered_pilots(mode: RobustnessMode) -> ScatteredPilotParams {
    match mode {
        RobustnessMode::A => ScatteredPilotParams {
            freq_int: 4,
            time_int: 5,
            x: 4,
            y: 5,
            k0: 2,
            model: GainPhaseModel::Wzq256,
            wz_cols: 3,
            w: &[228, 341, 455, 455, 569, 683, 683, 796, 910, 910, 0, 114, 114, 228, 341],
            z: &[0, 81, 248, 18, 106, 106, 122, 116, 31, 129, 129, 39, 33, 32, 111],
            q: 36,
            r: &[],
            q1024: &[],
        },
        RobustnessMode::B => ScatteredPilotParams {
            freq_int: 2,
            time_int: 3,
            x: 2,
            y: 3,
            k0: 1,
            model: GainPhaseModel::Wzq256,
            wz_cols: 5,
            w: &[512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512],
            z: &[0, 57, 164, 64, 12, 168, 255, 161, 106, 118, 25, 232, 132, 233, 38],
            q: 12,
            r: &[],
            q1024: &[],
        },
        RobustnessMode::C => ScatteredPilotParams {
            freq_int: 2,
            time_int: 2,
            x: 2,
            y: 2,
            k0: 1,
            model: GainPhaseModel::Wzq256,
            wz_cols: 10,
            w: &[
                465, 372, 279, 186, 93, 0, 931, 838, 745, 652, //
                931, 838, 745, 652, 559, 465, 372, 279, 186, 93,
            ],
            z: &[
                0, 76, 29, 76, 9, 190, 161, 248, 33, 108, //
                179, 178, 83, 253, 127, 105, 101, 198, 250, 145,
            ],
            q: 12,
            r: &[],
            q1024: &[],
        },
        RobustnessMode::D => ScatteredPilotParams {
            freq_int: 1,
            time_int: 3,
            x: 1,
            y: 3,
            k0: 1,
            model: GainPhaseModel::Wzq256,
            wz_cols: 8,
            w: &[
                366, 439, 512, 585, 658, 731, 805, 878, //
                731, 805, 878, 951, 0, 73, 146, 219, //
                73, 146, 219, 293, 366, 439, 512, 585,
            ],
            z: &[
                0, 240, 17, 60, 220, 38, 151, 101, //
                110, 7, 78, 82, 175, 150, 106, 25, //
                165, 7, 252, 124, 253, 177, 197, 142,
            ],
            q: 14,
            r: &[],
            q1024: &[],
        },
        // Mode E (DRM+): k = 2 + 4·(s mod 4) + 16·p; period 4, step 4, 13–14 carriers.
        // (x, y, k₀) = (4, 4, 2) — settled by the signal, which also settles the layout-mode
        // scramble of table 60.
        RobustnessMode::E => ScatteredPilotParams {
            freq_int: 4,
            time_int: 4,
            x: 4,
            y: 4,
            k0: 2,
            model: GainPhaseModel::Rzq1024,
            wz_cols: 10,
            w: &[],
            z: &GAIN_E_Z,
            q: 0,
            r: &GAIN_E_R,
            q1024: &GAIN_E_Q,
        },
    }
}

/// Carrier indices of the boosted (power 4) gain references at the band edges,
/// indexed by spectrum occupancy (§8.4.4.3.2, table 92). Zero rows are undefined
/// combinations.
pub const fn boosted_pilots(mode: RobustnessMode, so: usize) -> [i16; 4] {
    // Mode E, occupancy 0 only (§8.4.4.2, table 59): the four edge carriers.
    if matches!(mode, RobustnessMode::E) {
        return [-106, -102, 102, 106];
    }
    const A: [[i16; 4]; 6] = [
        [2, 6, 98, 102],
        [2, 6, 110, 114],
        [-102, -98, 98, 102],
        [-114, -110, 110, 114],
        [-98, -94, 310, 314],
        [-110, -106, 346, 350],
    ];
    const B: [[i16; 4]; 6] = [
        [1, 3, 89, 91],
        [1, 3, 101, 103],
        [-91, -89, 89, 91],
        [-103, -101, 101, 103],
        [-87, -85, 277, 279],
        [-99, -97, 309, 311],
    ];
    const C: [[i16; 4]; 6] = [
        [0; 4],
        [0; 4],
        [0; 4],
        [-69, -67, 67, 69],
        [0; 4],
        [-67, -65, 211, 213],
    ];
    const D: [[i16; 4]; 6] = [
        [0; 4],
        [0; 4],
        [0; 4],
        [-44, -43, 43, 44],
        [0; 4],
        [-43, -42, 134, 135],
    ];
    match mode {
        RobustnessMode::A => A[so],
        RobustnessMode::B => B[so],
        RobustnessMode::C => C[so],
        RobustnessMode::D => D[so],
        // Handled by the early return above (mode E, occupancy 0).
        RobustnessMode::E => [-106, -102, 102, 106],
    }
}

/// Average power of data cells, normal pilots and boosted pilots (§8.4.1).
pub const DATA_CELL_POWER: f64 = 1.0;
pub const PILOT_POWER: f64 = 2.0;
pub const BOOSTED_PILOT_POWER: f64 = 4.0;

/// AFS reference cells of robustness mode E (§8.4.5, table 61), by **super-frame**
/// OFDM symbol: s = 4 (fifth symbol of the first frame) and s = 39 of the fourth frame,
/// i.e. super-frame index 3·40 + 39 = 159. Carriers are every fourth one (k ≡ 2 mod 4),
/// 54 in all.
///
/// These cells are AFS *and* gain references at once where they coincide with a
/// scattered pilot (13 of the 54; the note under table 61), so the phase is the AFS
/// one and the amplitude the gain reference's. They are therefore neither SDC nor MSC
/// cells — ignoring them left mode E with 977 SDC cells instead of the standard's 936
/// and 7470 MSC cells instead of 7460 (table 25, table 45), which is why the DRM+ SDC
/// never decoded.
pub const fn afs_cells(mode: RobustnessMode, super_symbol: u8) -> &'static [(i16, u16)] {
    if !matches!(mode, RobustnessMode::E) {
        return &[];
    }
    match super_symbol {
        4 => &AFS_S4,
        // s = 39 of the fourth frame (§8.4.5.0).
        159 => &AFS_S39,
        _ => &[],
    }
}

/// Phase of the AFS cells at symbol 4 (§8.4.5.1, table 61).
static AFS_S4: [(i16, u16); 54] = [
    (-106, 134), (-102, 866), (-98, 588), (-94, 325), (-90, 77), (-86, 868), (-82, 649),
    (-78, 445), (-74, 256), (-70, 82), (-66, 946), (-62, 801), (-58, 671), (-54, 556),
    (-50, 455), (-46, 369), (-42, 298), (-38, 242), (-34, 200), (-30, 173), (-26, 161),
    (-22, 164), (-18, 181), (-14, 213), (-10, 260), (-6, 322), (-2, 398), (2, 489), (6, 595),
    (10, 716), (14, 851), (18, 1001), (22, 142), (26, 322), (30, 516), (34, 725), (38, 949),
    (42, 164), (46, 417), (50, 685), (54, 968), (58, 242), (62, 554), (66, 881), (70, 199),
    (74, 556), (78, 927), (82, 289), (86, 690), (90, 82), (94, 512), (98, 957), (102, 393),
    (106, 868),
];

/// Phase of the AFS cells at symbol 39 (§8.4.5.1, table 61). Symmetric about k = −2.
static AFS_S39: [(i16, u16); 54] = [
    (-106, 115), (-102, 135), (-98, 194), (-94, 293), (-90, 431), (-86, 608), (-82, 825),
    (-78, 57), (-74, 353), (-70, 688), (-66, 38), (-62, 452), (-58, 905), (-54, 373),
    (-50, 905), (-46, 452), (-42, 39), (-38, 689), (-34, 354), (-30, 59), (-26, 827),
    (-22, 610), (-18, 433), (-14, 295), (-10, 197), (-6, 138), (-2, 118), (2, 138), (6, 197),
    (10, 295), (14, 433), (18, 610), (22, 827), (26, 59), (30, 354), (34, 689), (38, 39),
    (42, 452), (46, 905), (50, 373), (54, 905), (58, 452), (62, 38), (66, 688), (70, 353),
    (74, 57), (78, 825), (82, 608), (86, 431), (90, 293), (94, 194), (98, 135), (102, 115),
    (106, 134),
];

// ---------------------------------------------------------------------------------
// Channel coding (§7.3)
// ---------------------------------------------------------------------------------

/// Constraint length of the DRM convolutional mother code (rate 1/6, §7.3.2).
pub const CONSTRAINT_LENGTH: usize = 7;

/// Number of trellis states.
pub const NUM_STATES: usize = 1 << (CONSTRAINT_LENGTH - 1);

/// Generator polynomials of the mother code, as tap masks over the shift register
/// where bit `i` holds the input bit delayed by `i` (bit 0 = current input).
/// The standard gives them as octal 133, 171, 145, 133, 171, 145 with the current input
/// as the most significant tap; these are the bit-reversed forms (Dream's
/// `byGeneratorMatrix`). The mother code is rate **1/6** (§7.3.2): outputs b₃..b₅ repeat
/// the polynomials of b₀..b₂, and only the 1/6 rate keeps them. Every rate modes A–D use
/// punctures them away, which is why four generators sufficed until mode E needed 1/6
/// (its 16-QAM protection level 0, table 31).
pub const GENERATORS: [u8; NUM_MOTHER_OUTPUTS] =
    [0o155, 0o117, 0o123, 0o155, 0o117, 0o123];

/// Outputs of the rate-1/6 mother code (§7.3.2). The puncturing masks are `u8`, so all
/// six fit in one mask.
pub const NUM_MOTHER_OUTPUTS: usize = 6;

/// Which of the four mother-code outputs b0..b3 are transmitted for one input bit.
/// Bit `i` of the mask set ⇒ output `b_i` is kept (Dream's `PP_TYPE_*`).
pub type PunctureMask = u8;

/// The code rates used by DRM, index 0..=12 (§7.3.1, table 27):
/// 1/4, 3/10, 1/3, 4/11, 1/2, 4/7, 3/5, 2/3, 8/11, 3/4, 4/5, 7/8, 8/9.
///
/// Indices 0..=12 are the rates modes A–D use and must not move: they are what the SDC
/// and FAC signal, and [`MSC16_SM`] and friends are index tables into them.
///
/// Robustness mode E additionally uses **1/6** and **2/5** (tables 29 and 31), which
/// modes A–D never do. They are appended at [`RATE_1_6`] and [`RATE_2_5`] so the existing
/// numbering stays put — the two extra rates are reached by name, not by index.
#[derive(Debug, Clone, Copy)]
pub struct CodeRate {
    /// Numerator RX (input bits per puncturing period).
    pub rx: usize,
    /// Denominator RY (output bits per puncturing period).
    pub ry: usize,
    /// Puncturing masks, one per input bit of the period (length `rx`).
    pub pattern: &'static [PunctureMask],
}

impl CodeRate {
    pub fn rate(&self) -> f64 {
        self.rx as f64 / self.ry as f64
    }
}

const P111111: PunctureMask = 0b111111;
const P0111: PunctureMask = 0b0111;
const P0011: PunctureMask = 0b0011;
const P0001: PunctureMask = 0b0001;
const P0101: PunctureMask = 0b0101;
const P1111: PunctureMask = 0b1111;
/// 2/5, first of the two input bits: b₀ b₁ b₂ (table 27).
const P111000: PunctureMask = 0b000111;
/// 2/5, second input bit: b₀ b₁ only.
const P110000: PunctureMask = 0b000011;

pub const CODE_RATES: [CodeRate; 15] = [
    CodeRate { rx: 1, ry: 4, pattern: &[P1111] },
    CodeRate { rx: 3, ry: 10, pattern: &[P1111, P0111, P0111] },
    CodeRate { rx: 1, ry: 3, pattern: &[P0111] },
    CodeRate { rx: 4, ry: 11, pattern: &[P0111, P0111, P0111, P0011] },
    CodeRate { rx: 1, ry: 2, pattern: &[P0011] },
    CodeRate { rx: 4, ry: 7, pattern: &[P0011, P0101, P0011, P0001] },
    CodeRate { rx: 3, ry: 5, pattern: &[P0011, P0001, P0011] },
    CodeRate { rx: 2, ry: 3, pattern: &[P0011, P0001] },
    CodeRate {
        rx: 8,
        ry: 11,
        pattern: &[P0011, P0001, P0001, P0011, P0001, P0001, P0011, P0001],
    },
    CodeRate { rx: 3, ry: 4, pattern: &[P0011, P0001, P0001] },
    CodeRate { rx: 4, ry: 5, pattern: &[P0011, P0001, P0001, P0001] },
    CodeRate { rx: 7, ry: 8, pattern: &[P0011, P0001, P0001, P0001, P0001, P0001, P0001] },
    CodeRate {
        rx: 8,
        ry: 9,
        pattern: &[P0011, P0001, P0001, P0001, P0001, P0001, P0001, P0001],
    },
    // 1/6 — the unpunctured mother code, all six outputs. Mode E 16-QAM level 0
    // (table 31: R₀ = 1/6); modes A–D never use it.
    CodeRate { rx: 1, ry: 6, pattern: &[P111111] },
    // 2/5 — mode E 4-QAM protection level 2 (table 29: R₀ = 2/5). Modes A–D never use it.
    CodeRate { rx: 2, ry: 5, pattern: &[P111000, P110000] },
];

/// Index of the 1/6 mother rate in [`CODE_RATES`] (mode E 16-QAM level 0).
pub const RATE_1_6: usize = 13;
/// Index of the 2/5 rate in [`CODE_RATES`] (mode E 4-QAM protection level 2).
pub const RATE_2_5: usize = 14;

/// Puncturing patterns for the six tail bits, selected by
/// `r_p = (2·N₂ − 12) − RY·⌊(2·N₂ − 12)/RY⌋` (§7.3.1, table 68).
pub const TAIL_PATTERNS: [[PunctureMask; 6]; 12] = [
    [P0011, P0011, P0011, P0011, P0011, P0011],
    [P0111, P0011, P0011, P0011, P0011, P0011],
    [P0111, P0011, P0011, P0111, P0011, P0011],
    [P0111, P0111, P0011, P0111, P0011, P0011],
    [P0111, P0111, P0011, P0111, P0111, P0011],
    [P0111, P0111, P0111, P0111, P0111, P0011],
    [P0111, P0111, P0111, P0111, P0111, P0111],
    [P1111, P0111, P0111, P0111, P0111, P0111],
    [P1111, P0111, P0111, P1111, P0111, P0111],
    [P1111, P1111, P0111, P1111, P0111, P0111],
    [P1111, P1111, P0111, P1111, P0111, P1111],
    [P1111, P1111, P1111, P1111, P0111, P1111],
];

/// MSC 16-QAM SM code-rate indices per protection level: [R0, R1] and RYlcm
/// (§7.5.1, table 70).
pub const MSC16_SM: [([usize; 2], usize); 2] = [([2, 7], 3), ([4, 9], 4)];

/// MSC 16-QAM code-rate indices for robustness mode E: [R0, R1] and RYlcm, protection
/// levels 0..=3 (table 31). Levels 2 and 3 are the same rates as [`MSC16_SM`]'s two
/// levels — mode E simply adds the two more-robust ones above them (1/6 and 1/4).
///
/// R₀ = 1/6 is the **unpunctured mother code**, so these are the only mode E levels that
/// keep mother outputs b₄ and b₅ (see [`GENERATORS`]).
pub const MSC16_SM_E: [([usize; 2], usize); 4] =
    [([RATE_1_6, 4], 6), ([0, 5], 28), ([2, 7], 3), ([4, 9], 4)];

/// MSC 4-QAM code-rate indices for robustness mode E, protection levels 0..=3
/// (table 29: R₀ = 1/4, 1/3, 2/5, 1/2). There is one level, so the whole rate is
/// that level's rate and no RYlcm is needed.
pub const MSC4_E: [usize; 4] = [0, 2, RATE_2_5, 4];

/// SDC 16-QAM code-rate indices [R0, R1] (overall rate 0.5).
pub const SDC16_RATES: [usize; 2] = [2, 7];
/// SDC 4-QAM code-rate index for modes A–D (rate 0.5).
pub const SDC4_RATE: usize = 4;
/// SDC 4-QAM code rates for robustness mode E, protection levels 0..=1
/// (table 38: 0.5 and 0.25). The FAC's SDC-mode bit selects between them.
pub const SDC4_E: [usize; 2] = [4, 0];
/// FAC 4-QAM code-rate index (rate 0.6).
/// FAC code-rate index for modes A-D (3/5). Mode E uses 1/4: see `fec::mlc::fac_rate_index`.
pub const FAC_RATE: usize = 6;

/// MSC 64-QAM SM code-rate indices per protection level: [R0, R1, R2] and RYlcm
/// (§7.5.1, table 71).
pub const MSC64_SM: [([usize; 3], usize); 4] =
    [([0, 4, 9], 4), ([2, 7, 10], 15), ([4, 9, 11], 8), ([7, 10, 12], 45)];

/// MSC 64-QAM HMsym code-rate indices: [R0 (VSPP), R1, R2] and RYlcm (table 72).
pub const MSC64_HMSYM: [([usize; 3], usize); 4] =
    [([4, 1, 6], 10), ([5, 3, 8], 11), ([6, 5, 11], 56), ([7, 7, 12], 9)];

/// MSC 64-QAM HMmix code-rate indices: [R0Re (VSPP), R0Im, R1Re, R1Im, R2Re, R2Im]
/// and RYlcm (table 73).
pub const MSC64_HMMIX: [([usize; 6], usize); 4] = [
    ([4, 0, 1, 4, 6, 9], 20),
    ([5, 2, 3, 7, 8, 10], 165),
    ([6, 4, 5, 9, 11, 11], 56),
    ([7, 7, 7, 10, 12, 12], 45),
];

/// Bit-interleaver constants t₀ for the two interleaver types (§7.3.3).
pub const BIT_INTERLEAVER_T0: [usize; 2] = [13, 21];

// ---------------------------------------------------------------------------------
// QAM mapping (§7.4)
// ---------------------------------------------------------------------------------

/// Normalised PAM amplitudes per axis. Index bits are the level bits with level 0 as
/// the most significant bit (e.g. 64-QAM: `idx = y0<<2 | y1<<1 | y2`).
pub const QAM4: [f64; 2] = [std::f64::consts::FRAC_1_SQRT_2, -std::f64::consts::FRAC_1_SQRT_2];

pub const QAM16: [f64; 4] = [0.948_683_298_0, -0.316_227_766_0, 0.316_227_766_0, -0.948_683_298_0];

pub const QAM64_SM: [f64; 8] = [
    1.080_123_449_7,
    -0.154_303_349_9,
    0.462_910_049_8,
    -0.771_516_749_8,
    0.771_516_749_8,
    -0.462_910_049_8,
    0.154_303_349_9,
    -1.080_123_449_7,
];

pub const QAM64_HMSYM: [f64; 8] = [
    1.080_123_449_7,
    0.462_910_049_8,
    0.771_516_749_8,
    0.154_303_349_9,
    -0.154_303_349_9,
    -0.771_516_749_8,
    -0.462_910_049_8,
    -1.080_123_449_7,
];

/// HMmix uses different tables for the in-phase and quadrature axes.
pub const QAM64_HMMIX_RE: [f64; 8] = [
    1.080_123_449_7,
    0.462_910_049_8,
    0.771_516_749_8,
    0.154_303_349_9,
    -0.154_303_349_9,
    -0.771_516_749_8,
    -0.462_910_049_8,
    -1.080_123_449_7,
];
pub const QAM64_HMMIX_IM: [f64; 8] = [
    1.080_123_449_7,
    -0.154_303_349_9,
    0.462_910_049_8,
    -0.771_516_749_8,
    0.771_516_749_8,
    -0.462_910_049_8,
    0.154_303_349_9,
    -1.080_123_449_7,
];

/// MSC dummy cells appended at the end of a super frame when the number of MSC cells
/// is not a multiple of three (§7.7): alternating (1+j) and (1−j) scaled to the
/// innermost constellation point.
pub const DUMMY_CELL_64QAM: f64 = 0.154_303_349_9;
pub const DUMMY_CELL_16QAM: f64 = 0.316_227_766_0;
