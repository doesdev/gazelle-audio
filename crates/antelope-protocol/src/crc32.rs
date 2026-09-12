//! CRC32 (IEEE 802.3) for cyclic-report integrity checks.
//!
//! Recovered from `refs/decompiled/manager/antelope_dev_base.py`
//! (`_cyclic_report_crc32_check`): the device stores the CRC32 of the report
//! payload (everything after the 16-byte header) in the `seq` field of cyclic
//! reports. `zlib.crc32` is the reference implementation.

/// Compute the IEEE CRC32 of `data`, matching Python's `zlib.crc32`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut crc = n as u32;
        let mut c = 0;
        while c < 8 {
            if crc & 1 != 0 {
                crc = 0xEDB8_8320 ^ (crc >> 1);
            } else {
                crc >>= 1;
            }
            c += 1;
        }
        table[n] = crc;
        n += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // zlib.crc32(b"") == 0
        assert_eq!(crc32(b""), 0);
        // zlib.crc32(b"123456789") == 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // zlib.crc32(b"The quick brown fox jumps over the lazy dog") == 0x414FA339
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }
}
