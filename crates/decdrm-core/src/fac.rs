//! Fast Access Channel content (ES 201 980 §6.3): channel parameters of the
//! transmission plus the parameters of one service per frame.

use crate::bits::{BitReader, BitWriter};
use crate::fec::crc::Crc;
use crate::fec::qam::Mapping;
use crate::params::{RobustnessMode, SpectrumOccupancy};

/// Interleaver depth of the MSC (§7.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interleaving {
    /// 2 s (5 multiplex frames).
    Long,
    /// 400 ms (1 multiplex frame).
    Short,
}

/// MSC constellation (FAC "MSC mode" field).
///
/// The field's meaning depends on the FAC's RM flag (§6.3.3), so the same two bit
/// patterns name different constellations in mode E than in modes A–D — see
/// [`MscMode::from_bits`]. `Qam4` therefore only ever comes from a mode E signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MscMode {
    /// Mode E only (RM flag = 1, field `11`).
    Qam4,
    Qam64Sm,
    Qam64HmMix,
    Qam64HmSym,
    Qam16Sm,
}

impl MscMode {
    pub fn mapping(self) -> Mapping {
        match self {
            Self::Qam4 => Mapping::Qam4,
            Self::Qam64Sm => Mapping::Qam64Sm,
            Self::Qam64HmMix => Mapping::Qam64HmMix,
            Self::Qam64HmSym => Mapping::Qam64HmSym,
            Self::Qam16Sm => Mapping::Qam16,
        }
    }

    /// Decode the 2-bit MSC-mode field, whose meaning is **dependent on the RM flag**
    /// (§6.3.3): modes A–D code `00` 64-QAM and `11` 16-QAM, while mode E codes `00`
    /// 16-QAM and `11` **4-QAM**. Reading one table for both — as this did — reports the
    /// constellation one step off on every mode E signal, which is why a real 4-QAM DRM+
    /// recording decoded as 16-QAM and a real 16-QAM one as 64-QAM. `01` and `10` are
    /// reserved in mode E (DecDRM uses them for the two hierarchical mappings in A–D);
    /// returning `None` there keeps a corrupted field from being taken for a constellation
    /// it is not.
    pub fn from_bits(v: u32, rm_flag: bool) -> Option<Self> {
        match (v & 3, rm_flag) {
            (0, false) => Some(Self::Qam64Sm),
            (0, true) => Some(Self::Qam16Sm),
            (3, false) => Some(Self::Qam16Sm),
            (3, true) => Some(Self::Qam4),
            (1, false) => Some(Self::Qam64HmMix),
            (2, false) => Some(Self::Qam64HmSym),
            _ => None,
        }
    }

    /// Encode the 2-bit MSC-mode field for a transmission whose RM flag is `rm_flag`.
    pub fn bits(self, rm_flag: bool) -> u32 {
        match (self, rm_flag) {
            (Self::Qam16Sm, true) => 0,
            (Self::Qam4, true) => 3,
            (Self::Qam64Sm, false) => 0,
            (Self::Qam64HmMix, false) => 1,
            (Self::Qam64HmSym, false) => 2,
            (Self::Qam16Sm, false) => 3,
            // A 4-QAM MSC exists only in mode E and a 64-QAM one only in A–D; the
            // mismatched pairs cannot be signalled, so they fall back to the mode's
            // 16-QAM code as the only value both tables share.
            (Self::Qam4, false) | (Self::Qam64Sm | Self::Qam64HmMix | Self::Qam64HmSym, true) => 3,
        }
    }

    pub fn is_hierarchical(self) -> bool {
        matches!(self, Self::Qam64HmMix | Self::Qam64HmSym)
    }
}

/// SDC constellation and protection (FAC "SDC mode" field).
///
/// Like the MSC mode, the field's meaning depends on the RM flag (§6.3.3). In modes A–D
/// it picks the constellation (16-QAM or 4-QAM, both rate 1/2); in **mode E it is a
/// protection level**: `0` is 4-QAM at rate 1/2 and `1` is 4-QAM at rate **1/4**. That is
/// the SDC protection the SDC's own data does not carry, so losing it silently decodes
/// mode E's SDC with the wrong code rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SdcMode {
    Qam16,
    Qam4,
    /// Mode E only (RM flag = 1, field `1`): 4-QAM at rate 1/4.
    Qam4Rate25,
}

impl SdcMode {
    pub fn mapping(self) -> Mapping {
        match self {
            Self::Qam16 => Mapping::Qam16,
            Self::Qam4 | Self::Qam4Rate25 => Mapping::Qam4,
        }
    }

    /// SDC protection level as [`crate::fec::mlc::MlcParams::sdc_for`] takes it: modes A–D
    /// have a single level (0), mode E's bit selects between its two (table 38).
    pub fn protection(self) -> usize {
        usize::from(self == Self::Qam4Rate25)
    }

    /// Decode the 1-bit SDC-mode field for a transmission whose RM flag is `rm_flag`
    /// (§6.3.3). Mode A–D: 0 = 16-QAM, 1 = 4-QAM. Mode E: 0 = 4-QAM rate 1/2,
    /// 1 = 4-QAM rate 1/4.
    pub fn from_bits(bit: u32, rm_flag: bool) -> Self {
        match (bit & 1 == 1, rm_flag) {
            (false, false) => Self::Qam16,
            (true, false) => Self::Qam4,
            (false, true) => Self::Qam4,
            (true, true) => Self::Qam4Rate25,
        }
    }

    /// Encode the 1-bit SDC-mode field for a transmission whose RM flag is `rm_flag` —
    /// the inverse of [`Self::from_bits`].
    pub fn bits(self, rm_flag: bool) -> u32 {
        match (self, rm_flag) {
            (Self::Qam16, false) => 0,
            (Self::Qam4, false) => 1,
            // Mode E: `0` is 4-QAM at rate 1/2, `1` is 4-QAM at rate 1/4.
            (Self::Qam4, true) => 0,
            (Self::Qam4Rate25, true) => 1,
            // The two mismatched pairs cannot be signalled — mode E's SDC is always 4-QAM
            // (§8.5.3.2) and modes A–D have no rate-1/4 level — so each falls back to the
            // rate-1/2 4-QAM code that both its constellation and its rate agree with.
            (Self::Qam16, true) | (Self::Qam4Rate25, false) => 0,
        }
    }
}

/// Channel parameters (first 20 bits of the FAC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelParams {
    pub enhancement: bool,
    /// Frame index within the super frame (0..=2).
    pub frame_index: u8,
    /// The SDC AFS index is valid (identity field 0; identity 3 marks the first
    /// frame with an invalid AFS index). Only meaningful when `frame_index == 0`.
    pub afs_valid: bool,
    pub occupancy: SpectrumOccupancy,
    pub interleaving: Interleaving,
    pub msc_mode: MscMode,
    pub sdc_mode: SdcMode,
    pub num_audio: u8,
    pub num_data: u8,
    pub reconfiguration_index: u8,
    pub toggle: bool,
}

/// Service parameters (44 bits) of the service signalled in this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceParams {
    pub service_id: u32,
    pub short_id: u8,
    pub audio_ca: bool,
    pub language: u8,
    pub is_data: bool,
    /// Programme type (audio) or application identifier (data).
    pub descriptor: u8,
    pub data_ca: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fac {
    pub channel: ChannelParams,
    pub service: ServiceParams,
}

/// Number-of-services code (§6.3.3, table 60): (audio, data) → 4-bit code.
const NUM_SERVICES: [[i8; 5]; 5] = [
    [-1, 1, 2, 3, 15],
    [4, 5, 6, 7, -1],
    [8, 9, 10, -1, -1],
    [12, 13, -1, -1, -1],
    [0, -1, -1, -1, -1],
];

fn decode_num_services(code: u32) -> Option<(u8, u8)> {
    for (a, row) in NUM_SERVICES.iter().enumerate() {
        for (d, &v) in row.iter().enumerate() {
            if v as i32 == code as i32 {
                return Some((a as u8, d as u8));
            }
        }
    }
    None
}

/// Language names of the 4-bit FAC language code (§6.3.4, table 62).
pub const LANGUAGES: [&str; 16] = [
    "No language specified",
    "Arabic",
    "Bengali",
    "Chinese (Mandarin)",
    "Dutch",
    "English",
    "French",
    "German",
    "Hindi",
    "Japanese",
    "Javanese",
    "Korean",
    "Portuguese",
    "Russian",
    "Spanish",
    "Other language",
];

/// Programme type names (§6.3.4, table 63).
pub const PROGRAMME_TYPES: [&str; 32] = [
    "No programme type",
    "News",
    "Current Affairs",
    "Information",
    "Sport",
    "Education",
    "Drama",
    "Culture",
    "Science",
    "Varied",
    "Pop Music",
    "Rock Music",
    "Easy Listening Music",
    "Light Classical",
    "Serious Classical",
    "Other Music",
    "Weather/meteorology",
    "Finance/Business",
    "Children's programmes",
    "Social Affairs",
    "Religion",
    "Phone In",
    "Travel",
    "Leisure",
    "Jazz Music",
    "Country Music",
    "National Music",
    "Oldies Music",
    "Folk Music",
    "Documentary",
    "Not used",
    "Not used",
];

impl Fac {
    /// Parse a decoded FAC block, one bit per byte. Returns `None` if the CRC fails or a
    /// field is invalid.
    ///
    /// Two widths exist (§7.2.1.2): **72 bits** for robustness modes A–D (20 channel bits
    /// + one 44-bit service parameter set + 8 CRC) and **116 bits** for mode E (the same
    /// channel bits + **two** service parameter sets + 8 CRC — §6.3's "two sets of service
    /// parameters"), the CRC covering everything before it. The width is taken from the
    /// slice, so a caller that knows the mode does not have to say so — which also fixes the
    /// super frame's length the Identity field is read against (3 frames for A–D, 4 for E).
    pub fn parse(bits: &[u8]) -> Option<Self> {
        // 64 + 8 for A–D, 108 + 8 for mode E.
        let (payload, crc_at, frames) = if bits.len() >= 116 {
            (108, 108, 4)
        } else if bits.len() >= 72 {
            (64, 64, 3)
        } else {
            return None;
        };
        let mut crc = Crc::crc8();
        for &b in &bits[..payload] {
            crc.add_bit(b & 1 == 1);
        }
        if bits.len() >= 116 {
            // Four reserved bits sit between the information and the CRC. They are
            // **included in the CRC as zeros but not transmitted**: the block is nominally
            // 20 + 88 service + 4 reserved + 8 CRC = 120 bits, and the 116 transmitted bits
            // are [0..108] ++ [112..120]. This is what JvanKatwijk's decoder does by hand
            // (`facBits[120-1-i] = facBits[120-5-i]`) before its CRC over 120, and it is
            // invisible to a DecDRM-to-DecDRM loopback because both ends shared the
            // mistaken 108-bit coverage.
            for _ in 0..4 {
                crc.add_bit(false);
            }
        }
        let mut r = BitReader::from_bits(bits);
        let rx_crc = {
            let mut t = BitReader::from_bits(&bits[crc_at..crc_at + 8]);
            t.read(8)
        };
        if crc.value() != rx_crc {
            return None;
        }
        let enhancement = r.read(1) == 1;
        let identity = r.read(2) as u8;
        // The channel parameters are **20 bits in every mode**: Base/Enhancement (1),
        // Identity (2), **RM flag (1)**, Spectrum occupancy (**3**), Interleaver depth (1),
        // MSC mode (2), SDC mode (1), services (4), Reconfiguration index (3), Toggle (1),
        // rfu (1) — §6.3.3. The RM flag says whether the signal is mode E (`1`, two sets of
        // service parameters) or A–D (`0`, one set). Reading the occupancy as 4 bits, as
        // DecDRM did, swallows that flag as the value's top bit: mode E's `1 000` becomes 8,
        // which is not a valid occupancy, so every real mode E frame was rejected. DecDRM's
        // own transmitter wrote the same wrong width, so only a loopback ever passed.
        //
        // The flag also **re-interprets** four of the fields that follow it — the
        // interleaver depth, the MSC mode, the SDC mode and the toggle flag (§6.3.3, "the
        // interpretation of ... is dependent on the value of the RM flag"). Each is decoded
        // through the flag below rather than with the mode A–D table, which is what the
        // earlier code did and why a real 4-QAM DRM+ signal read as 16-QAM.
        let rm_flag = r.read(1) == 1;
        let occ_raw = r.read(3) as u8;
        let occupancy = SpectrumOccupancy::new(occ_raw)?;
        // RM flag = 1: 0 is the only defined value, 600 ms long interleaving (§6.3.3). The
        // bit is read unconditionally so the bit reader stays in step whichever table
        // applies.
        let intl_short = r.read(1) == 1;
        let interleaving =
            if intl_short && !rm_flag { Interleaving::Short } else { Interleaving::Long };
        let msc_mode = MscMode::from_bits(r.read(2), rm_flag)?;
        let sdc_mode = SdcMode::from_bits(r.read(1), rm_flag);
        let svc_raw = r.read(4);
        let (num_audio, num_data) = decode_num_services(svc_raw)?;
        let reconfiguration_index = r.read(3) as u8;
        let toggle = r.read(1) == 1;
        let _rfu = r.read(1);
        let service_id = r.read(24);
        let short_id = r.read(2) as u8;
        let audio_ca = r.read(1) == 1;
        let language = r.read(4) as u8;
        let is_data = r.read(1) == 1;
        let descriptor = r.read(5) as u8;
        let data_ca = r.read(1) == 1;
        Some(Fac {
            channel: ChannelParams {
                enhancement,
                frame_index: frame_index_of(identity as u32, toggle, frames),
                afs_valid: identity == 0,
                occupancy,
                interleaving,
                msc_mode,
                sdc_mode,
                num_audio,
                num_data,
                reconfiguration_index,
                toggle,
            },
            service: ServiceParams { service_id, short_id, audio_ca, language, is_data, descriptor, data_ca },
        })
    }

    /// The number of service parameter sets a FAC block of `len` bits carries (§6.3):
    /// one for modes A–D (72 bits), two for mode E (116 bits).
    pub fn service_sets(len: usize) -> usize {
        if len >= 116 { 2 } else { 1 }
    }

    /// Serialise to 72 bits including the CRC (transmitter side).
    pub fn to_bits(&self) -> Vec<u8> {
        self.to_bits_for(RobustnessMode::A)
    }

    /// Serialise this FAC for `mode` (§6.3, §7.2.1.2, §7.5.3).
    ///
    /// Modes A–D produce **72** bits: the channel parameters plus one service parameter set.
    /// Robustness mode E produces **116** bits: the same channel parameters plus **two**
    /// service parameter sets — which is what §6.3 means by "for robustness mode E, two sets
    /// of service parameters shall be transmitted". (20 + 2×44 + 8 CRC = 116.)
    ///
    /// This transmitter carries one service, so mode E sends that service's parameters twice,
    /// which is the single-service case of the two-set structure.
    pub fn to_bits_for(&self, mode: RobustnessMode) -> Vec<u8> {
        let mut bits = self.channel_and_service_bits(mode);
        if mode.is_drm_plus() {
            bits.extend_from_slice(&self.service_bits());
            // The four reserved bits participate in the CRC but are **not transmitted**
            // (§6.3.3): the block is 20 + 88 + 4 + 8 = 120 bits, and the 116 on the air are
            // [0..108] ++ [112..120]. Take the CRC over 120 and drop the four zeros.
            bits.extend_from_slice(&[0u8; 4]);
            append_crc8(&mut bits);
            bits.drain(108..112);
        } else {
            append_crc8(&mut bits);
        }
        bits
    }

    /// The channel parameters (20 bits) followed by one service parameter set (44 bits):
    /// 64 bits, no CRC. Every mode sends at least this.
    fn channel_and_service_bits(&self, mode: RobustnessMode) -> Vec<u8> {
        let c = &self.channel;
        let s = &self.service;
        let mut w = BitWriter::new();
        w.write(u32::from(c.enhancement), 1);
        // The Identity field encodes *position in the FAC cycle*, and a mode E super frame
        // has four blocks for three codes: derive it (and the toggle that separates the two
        // intermediate blocks) from the frame index and the mode, so a caller cannot write a
        // pair the receiver will read back as a different frame. See `identity_of`.
        let frames = mode.frames_per_superframe();
        w.write(identity_of(c.frame_index, c.afs_valid, frames), 2);
        // RM flag (1) then the 3-bit spectrum occupancy (§6.3.3). Mode E sets the flag and
        // its occupancy is always 0 (§8.1) — encoding it as a 4-bit occupancy, as this did,
        // sets the flag bit to the occupancy's high bit and gives a block real receivers
        // reject. The flag also selects the tables the next three fields are coded with,
        // so all of them take the same `is_drm_plus()` value.
        let rm = mode.is_drm_plus();
        w.write(u32::from(rm), 1);
        w.write(u32::from(c.occupancy.value()), 3);
        w.write(u32::from(!rm && c.interleaving == Interleaving::Short), 1);
        w.write(c.msc_mode.bits(rm), 2);
        w.write(c.sdc_mode.bits(rm), 1);
        let code = NUM_SERVICES
            .get(c.num_audio as usize)
            .and_then(|r| r.get(c.num_data as usize))
            .copied()
            .unwrap_or(-1);
        w.write(code.max(0) as u32, 4);
        w.write(u32::from(c.reconfiguration_index), 3);
        w.write(u32::from(toggle_for_frame(c.frame_index, frames)), 1);
        w.write(0, 1);
        let _ = s;
        let mut bits = w.into_bits();
        // The first service parameter set is present in every mode.
        bits.extend_from_slice(&self.service_bits());
        bits
    }

    /// One service parameter set: 44 bits (§6.3).
    fn service_bits(&self) -> Vec<u8> {
        let s = &self.service;
        let mut w = BitWriter::new();
        w.write(s.service_id & 0xFF_FFFF, 24);
        w.write(u32::from(s.short_id), 2);
        w.write(u32::from(s.audio_ca), 1);
        w.write(u32::from(s.language), 4);
        w.write(u32::from(s.is_data), 1);
        w.write(u32::from(s.descriptor), 5);
        w.write(u32::from(s.data_ca), 1);
        w.write(0, 6);
        w.into_bits()
    }
}

/// The transmission frame a FAC block belongs to, from its Identity field and — in mode E
/// only — its toggle flag (§6.3.3).
///
/// Identity marks a block's **position in the FAC cycle**, not a frame number: `00` and `11`
/// are the first block (AFS index valid / invalid), `01` an *intermediate* block and `10` the
/// last. Modes A–D have three frames and one intermediate block, so the identity is the frame
/// index (with `11` folding onto 0). **Mode E has four frames and two intermediate blocks** —
/// §6.3.3: "the toggle flag is set to zero for the first and third FAC block and to one for
/// the second and fourth" — so the toggle is what tells the two `01` blocks apart: frame 1
/// (toggle 1, after the first block) and frame 2 (toggle 0, before the last). A real mode E
/// transmitter sends `{0|3} 01 01 10`, i.e. frames 0, 1, 2, 3.
///
/// Reading the identity as the frame index (as this did) collapsed both intermediates onto
/// frame 1, so the frame count never reached 3: the super frame never completed, the SDC gate
/// on `frame_index == 0` almost never opened, and no multiplex frame was ever assembled.
fn frame_index_of(identity: u32, toggle: bool, frames: usize) -> u8 {
    match identity & 3 {
        0 | 3 => 0,
        2 => (frames.max(1) as u8) - 1,
        // Intermediate. Mode E's two are both `01`, told apart by the toggle; modes A–D
        // have a single one and fix the toggle at 0.
        _ if frames >= 4 => {
            if toggle {
                1
            } else {
                2
            }
        }
        _ => 1,
    }
}

/// The Identity field that encodes frame `frame_index`'s position in its FAC cycle, for a
/// transmission of `frames` frames per super frame (§6.3.3) — the inverse of
/// [`frame_index_of`]. `afs_valid` picks identity 0 over 3 for the first block, whose only
/// difference is whether the SDC's AFS index can be used.
fn identity_of(frame_index: u8, afs_valid: bool, frames: usize) -> u32 {
    if frame_index == 0 {
        return u32::from(!afs_valid) * 3; // 0 when the AFS index is valid, else 3
    }
    if frame_index as usize >= frames.max(1) - 1 {
        return 2; // the last block
    }
    1 // an intermediate block: 01 in every mode, its position given by the toggle
}

/// The toggle flag frame `frame_index` sends (§6.3.3). Mode E cycles it `0, 1, 0, 1` over its
/// four frames ("zero for the first and third FAC block ... one for the second and fourth");
/// modes A–D fix it at zero. It is a function of the frame's position, so deriving it here
/// rather than trusting a caller keeps it consistent with the Identity field.
pub fn toggle_for_frame(frame_index: u8, frames: usize) -> bool {
    frames >= 4 && frame_index % 2 == 1
}

/// The Identity and toggle fields a transmission of `frames` frames per super frame sends
/// for `frame_index` (§6.3.3) — the pair a transmitter must write and a receiver reads back.
pub fn identity_and_toggle(frame_index: u8, afs_valid: bool, frames: usize) -> (u32, bool) {
    (identity_of(frame_index, afs_valid, frames), toggle_for_frame(frame_index, frames))
}

/// Append the 8-bit FAC CRC to `bits` (§6, `Crc::crc8`).
fn append_crc8(bits: &mut Vec<u8>) {
    let mut crc = Crc::crc8();
    for &b in bits.iter() {
        crc.add_bit(b == 1);
    }
    let v = crc.value();
    for i in (0..8).rev() {
        bits.push(((v >> i) & 1) as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fac(mode: RobustnessMode, frame_index: u8, msc_mode: MscMode, sdc_mode: SdcMode) -> Fac {
        Fac {
            channel: ChannelParams {
                enhancement: false,
                frame_index,
                // The AFS index is validated only by the first FAC block of a super frame
                // (§6.3.3), so the flag is only ever set there.
                afs_valid: frame_index == 0,
                occupancy: if mode.is_drm_plus() {
                    SpectrumOccupancy::SO_0
                } else {
                    SpectrumOccupancy::SO_3
                },
                interleaving: Interleaving::Long,
                msc_mode,
                sdc_mode,
                num_audio: 1,
                num_data: 1,
                reconfiguration_index: 0,
                // Deliberately the *wrong* toggle for the frame: it is derived on the wire
                // (§6.3.3), so a round trip must correct it rather than echo it.
                toggle: true,
            },
            service: ServiceParams {
                service_id: 0x123456,
                short_id: 1,
                audio_ca: false,
                language: 5,
                is_data: false,
                descriptor: 1,
                data_ca: false,
            },
        }
    }

    #[test]
    fn roundtrip() {
        let f = fac(RobustnessMode::A, 2, MscMode::Qam64Sm, SdcMode::Qam16);
        let bits = f.to_bits();
        assert_eq!(bits.len(), 72);
        // Modes A–D fix the toggle at zero (§6.3.3), so the parsed block differs from the
        // constructed one in exactly that field.
        let expected = Fac { channel: ChannelParams { toggle: false, ..f.channel }, ..f };
        assert_eq!(Fac::parse(&bits), Some(expected));
        let mut bad = bits.clone();
        bad[10] ^= 1;
        assert_eq!(Fac::parse(&bad), None);
    }

    /// A mode E round trip must survive the fields the RM flag re-interprets: 4-QAM MSC,
    /// the rate-1/4 SDC, and the four-block identity/toggle cycle (§6.3.3). These are the
    /// fields that made a real DRM+ recording undecodable, and a mode E block is the only
    /// place they can be exercised — modes A–D have neither 4-QAM nor two intermediate
    /// blocks.
    #[test]
    fn roundtrip_mode_e() {
        for frame_index in 0..4u8 {
            let f = fac(RobustnessMode::E, frame_index, MscMode::Qam4, SdcMode::Qam4Rate25);
            let bits = f.to_bits_for(RobustnessMode::E);
            assert_eq!(bits.len(), 116, "mode E FAC is 116 bits");
            let parsed = Fac::parse(&bits).expect("mode E FAC parses");
            // The toggle is a function of the frame's position; every other field is echoed.
            let expected = Fac {
                channel: ChannelParams {
                    toggle: expected_toggle(frame_index),
                    ..f.channel
                },
                ..f
            };
            assert_eq!(parsed, expected, "frame {frame_index}");
            assert_eq!(parsed.channel.msc_mode, MscMode::Qam4);
            assert_eq!(parsed.channel.sdc_mode, SdcMode::Qam4Rate25);
            assert_eq!(parsed.channel.sdc_mode.protection(), 1, "rate 1/4");
        }
    }

    /// The identity field encodes a block's position in the FAC cycle, not a frame number:
    /// mode E has four frames for three codes, so the two intermediate blocks are both `01`
    /// and only the toggle separates them. This is what the collapse onto one intermediate
    /// got wrong, and it is checked against the literal sequence a real transmitter sends.
    #[test]
    fn mode_e_identity_cycle() {
        // The cycle a real DRM+ recording sends: {0|3} 01 01 10, toggle 0 1 0 1.
        let want = [(0u32, false), (1, true), (1, false), (2, true)];
        for (frame_index, &(identity, toggle)) in want.iter().enumerate() {
            let got_id = identity_of(frame_index as u8, true, 4);
            let got_tog = toggle_for_frame(frame_index as u8, 4);
            assert_eq!((got_id, got_tog), (identity, toggle), "frame {frame_index}");
            // And the receiver reads that pair back as the frame it came from.
            assert_eq!(frame_index_of(got_id, got_tog, 4), frame_index as u8);
        }
        // Modes A–D: three frames, one intermediate block, toggle fixed at zero.
        for frame_index in 0..3u8 {
            let id = identity_of(frame_index, true, 3);
            assert!(!toggle_for_frame(frame_index, 3));
            assert_eq!(frame_index_of(id, false, 3), frame_index);
        }
    }

    fn expected_toggle(frame_index: u8) -> bool {
        frame_index % 2 == 1
    }
}
