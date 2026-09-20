//! Sample types: what each code means, how wide it is, and how to carry one through a 32 bit
//! signed value without changing what it sounds like.
//!
//! Everything that crosses between two devices in this workspace is carried as [`i32`] at full
//! scale, because that is what both interfaces here report and it holds every other integer type
//! exactly. Only the copy in and the copy out convert.

/// One sample type: its code, the name a person reads, its width in bytes, and whether anything
/// here will convert it. The big endian types are listed so that a driver reporting one is named
/// rather than guessed at, and refused.
pub struct SampleType {
    pub code: i32,
    pub name: &'static str,
    pub bytes: usize,
    pub convertible: bool,
}

/// The little endian codes, which is everything a Windows driver reports.
pub const INT16_LSB: i32 = 16;
pub const INT24_LSB: i32 = 17;
pub const INT32_LSB: i32 = 18;
pub const FLOAT32_LSB: i32 = 19;
pub const FLOAT64_LSB: i32 = 20;
pub const INT32_LSB16: i32 = 24;
pub const INT32_LSB18: i32 = 25;
pub const INT32_LSB20: i32 = 26;
pub const INT32_LSB24: i32 = 27;

const TYPES: &[SampleType] = &[
    SampleType { code: 0, name: "Int16 big endian", bytes: 2, convertible: false },
    SampleType { code: 1, name: "Int24 big endian", bytes: 3, convertible: false },
    SampleType { code: 2, name: "Int32 big endian", bytes: 4, convertible: false },
    SampleType { code: 3, name: "Float32 big endian", bytes: 4, convertible: false },
    SampleType { code: 4, name: "Float64 big endian", bytes: 8, convertible: false },
    SampleType { code: 8, name: "Int32 big endian, 16 bits used", bytes: 4, convertible: false },
    SampleType { code: 9, name: "Int32 big endian, 18 bits used", bytes: 4, convertible: false },
    SampleType { code: 10, name: "Int32 big endian, 20 bits used", bytes: 4, convertible: false },
    SampleType { code: 11, name: "Int32 big endian, 24 bits used", bytes: 4, convertible: false },
    SampleType { code: INT16_LSB, name: "Int16 little endian", bytes: 2, convertible: true },
    SampleType { code: INT24_LSB, name: "Int24 little endian", bytes: 3, convertible: true },
    SampleType { code: INT32_LSB, name: "Int32 little endian", bytes: 4, convertible: true },
    SampleType { code: FLOAT32_LSB, name: "Float32 little endian", bytes: 4, convertible: true },
    SampleType { code: FLOAT64_LSB, name: "Float64 little endian", bytes: 8, convertible: true },
    SampleType { code: INT32_LSB16, name: "Int32 little endian, 16 bits used", bytes: 4, convertible: true },
    SampleType { code: INT32_LSB18, name: "Int32 little endian, 18 bits used", bytes: 4, convertible: true },
    SampleType { code: INT32_LSB20, name: "Int32 little endian, 20 bits used", bytes: 4, convertible: true },
    SampleType { code: INT32_LSB24, name: "Int32 little endian, 24 bits used", bytes: 4, convertible: true },
];

/// What this code is, or None when nothing here knows it.
pub fn describe(code: i32) -> Option<&'static SampleType> {
    TYPES.iter().find(|t| t.code == code)
}

/// The name a person reads, with the raw code in case it is one we do not know.
pub fn name(code: i32) -> String {
    match describe(code) {
        Some(t) => format!("{} (type {code})", t.name),
        None => format!("sample type {code}, which this software does not know"),
    }
}

/// How many bytes one sample of this type takes.
pub fn width(code: i32) -> Option<usize> {
    describe(code).map(|t| t.bytes)
}

/// Whether the copy path here can read and write this type.
pub fn convertible(code: i32) -> bool {
    describe(code).is_some_and(|t| t.convertible)
}

/// How far left a value of this type sits from the bottom of a 32 bit sample.
fn integer_shift(code: i32) -> Option<u32> {
    match code {
        INT16_LSB => Some(16),
        INT24_LSB => Some(8),
        INT32_LSB => Some(0),
        INT32_LSB16 => Some(16),
        INT32_LSB18 => Some(14),
        INT32_LSB20 => Some(12),
        INT32_LSB24 => Some(8),
        _ => None,
    }
}

/// The scale a float sample is held at: 1.0 is full scale.
const FULL_SCALE: f64 = 2_147_483_648.0;

/// Read one sample of `code` out of `bytes` as a full scale 32 bit value.
///
/// `bytes` must be at least [`width`] long. A type this does not convert reads as silence, which
/// is why nothing is opened at such a type in the first place.
#[inline]
pub fn read(code: i32, bytes: &[u8]) -> i32 {
    match code {
        INT16_LSB => (i16::from_le_bytes([bytes[0], bytes[1]]) as i32) << 16,
        INT24_LSB => {
            // Three bytes, low first, sign extended by landing them in the top of a 32 bit word.
            i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]])
        }
        INT32_LSB => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        FLOAT32_LSB => from_float(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64),
        FLOAT64_LSB => {
            let mut eight = [0u8; 8];
            eight.copy_from_slice(&bytes[..8]);
            from_float(f64::from_le_bytes(eight))
        }
        other => match integer_shift(other) {
            Some(shift) => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).wrapping_shl(shift),
            None => 0,
        },
    }
}

/// Write a full scale 32 bit value into `bytes` as one sample of `code`.
#[inline]
pub fn write(code: i32, value: i32, bytes: &mut [u8]) {
    match code {
        INT16_LSB => bytes[..2].copy_from_slice(&((value >> 16) as i16).to_le_bytes()),
        INT24_LSB => {
            let packed = value.to_le_bytes();
            bytes[..3].copy_from_slice(&packed[1..4]);
        }
        INT32_LSB => bytes[..4].copy_from_slice(&value.to_le_bytes()),
        FLOAT32_LSB => bytes[..4].copy_from_slice(&((value as f64 / FULL_SCALE) as f32).to_le_bytes()),
        FLOAT64_LSB => bytes[..8].copy_from_slice(&(value as f64 / FULL_SCALE).to_le_bytes()),
        other => match integer_shift(other) {
            Some(shift) => bytes[..4].copy_from_slice(&(value >> shift).to_le_bytes()),
            None => bytes.iter_mut().for_each(|b| *b = 0),
        },
    }
}

/// A float sample at full scale, clamped: a host that sends more than full scale is held at it
/// rather than wrapping round to the opposite sign, which is what an overflow would sound like.
#[inline]
fn from_float(value: f64) -> i32 {
    let scaled = value * FULL_SCALE;
    if scaled >= FULL_SCALE - 1.0 {
        i32::MAX
    } else if scaled <= -FULL_SCALE {
        i32::MIN
    } else {
        scaled as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_both_interfaces_report_is_a_copy() {
        let mut bytes = [0u8; 4];
        for value in [0i32, 1, -1, i32::MAX, i32::MIN, 12_345_678] {
            write(INT32_LSB, value, &mut bytes);
            assert_eq!(read(INT32_LSB, &bytes), value);
        }
        assert_eq!(width(INT32_LSB), Some(4));
    }

    #[test]
    fn narrower_integer_types_come_back_to_where_they_started() {
        let cases: &[(i32, usize, i32)] = &[
            (INT16_LSB, 2, 16),
            (INT24_LSB, 3, 8),
            (INT32_LSB16, 4, 16),
            (INT32_LSB18, 4, 14),
            (INT32_LSB20, 4, 12),
            (INT32_LSB24, 4, 8),
        ];
        for &(code, bytes_wide, shift) in cases {
            assert_eq!(width(code), Some(bytes_wide), "{code}");
            let mut bytes = [0u8; 8];
            // Only the bits this type carries survive a round trip, so start from a value that
            // has no bits below them.
            for value in [0i32, 1 << shift, -1 << shift, i32::MAX & !((1 << shift) - 1)] {
                write(code, value, &mut bytes);
                assert_eq!(read(code, &bytes), value, "type {code}, value {value}");
            }
        }
    }

    #[test]
    fn float_samples_hold_full_scale_rather_than_wrapping() {
        let mut bytes = [0u8; 8];
        write(FLOAT32_LSB, i32::MAX, &mut bytes);
        assert!(read(FLOAT32_LSB, &bytes) > i32::MAX / 2);
        write(FLOAT32_LSB, i32::MIN, &mut bytes);
        assert!(read(FLOAT32_LSB, &bytes) < i32::MIN / 2);
        // A float well past full scale clamps rather than turning into its own opposite.
        bytes[..4].copy_from_slice(&4.0f32.to_le_bytes());
        assert_eq!(read(FLOAT32_LSB, &bytes), i32::MAX);
        bytes[..4].copy_from_slice(&(-4.0f32).to_le_bytes());
        assert_eq!(read(FLOAT32_LSB, &bytes), i32::MIN);
    }

    #[test]
    fn silence_stays_silence_in_every_type_we_convert() {
        for t in TYPES.iter().filter(|t| t.convertible) {
            let mut bytes = [0xAAu8; 8];
            write(t.code, 0, &mut bytes);
            assert_eq!(read(t.code, &bytes), 0, "{}", t.name);
            assert!(bytes[..t.bytes].iter().all(|&b| b == 0), "{} left rubbish behind", t.name);
        }
    }

    #[test]
    fn a_type_we_do_not_know_is_named_and_refused() {
        assert!(describe(99).is_none());
        assert!(!convertible(99));
        assert!(!convertible(0), "big endian types are named but not converted");
        assert!(name(99).contains("99"));
        assert!(name(INT32_LSB).contains("Int32 little endian"));
    }
}
