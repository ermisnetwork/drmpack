//! Shared ISO Base Media File Format (ISOBMFF) box parsing and integrity validation.
//!
//! Provides utilities to inspect ISOBMFF box headers and verify completion of
//! initialization segments (`ftyp` + `moov`) and media segments (`moof` + `mdat`).

/// Extract the next ISOBMFF box from `data` at `offset`.
///
/// Returns a tuple of `(box_type, box_size)` where `box_type` is a 4-byte ASCII box identifier
/// and `box_size` is the total byte length of the box (including the header).
pub(crate) fn next_isobmff_box(data: &[u8], offset: usize) -> Option<(&[u8; 4], usize)> {
    if offset + 8 > data.len() {
        return None;
    }
    let size32 = u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]);
    let box_type: &[u8; 4] = data[offset + 4..offset + 8].try_into().ok()?;

    let box_size = match size32 {
        0 => data.len() - offset,
        1 => {
            if offset + 16 > data.len() {
                return None;
            }
            let s64 = u64::from_be_bytes(data[offset + 8..offset + 16].try_into().ok()?);
            if s64 < 16 || s64 > (data.len() - offset) as u64 {
                return None;
            }
            s64 as usize
        }
        s if s >= 8 => {
            let s = s as usize;
            if s > data.len() - offset {
                return None;
            }
            s
        }
        _ => return None,
    };

    Some((box_type, box_size))
}

/// Verify if `data` is a complete ISOBMFF media segment containing both `moof` and `mdat` boxes,
/// and all boxes are fully received.
pub(crate) fn is_complete_isobmff_media_segment(data: &[u8]) -> bool {
    let mut offset = 0;
    let mut has_moof = false;
    let mut has_mdat = false;

    while let Some((box_type, box_size)) = next_isobmff_box(data, offset) {
        if box_type == b"moof" {
            has_moof = true;
        } else if box_type == b"mdat" {
            has_mdat = true;
        }
        offset += box_size;
    }

    has_moof && has_mdat && !data.is_empty() && offset == data.len()
}

/// Verify if `data` is a complete ISOBMFF init segment containing both `ftyp` and `moov` boxes,
/// and all boxes are fully received.
pub(crate) fn is_complete_isobmff_init_segment(data: &[u8]) -> bool {
    let mut offset = 0;
    let mut has_ftyp = false;
    let mut has_moov = false;

    while let Some((box_type, box_size)) = next_isobmff_box(data, offset) {
        if box_type == b"ftyp" {
            has_ftyp = true;
        } else if box_type == b"moov" {
            has_moov = true;
        }
        offset += box_size;
    }

    has_ftyp && has_moov && !data.is_empty() && offset == data.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut buf = Vec::with_capacity(size as usize);
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(payload);
        buf
    }

    fn make_box_size_zero(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + payload.len());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(payload);
        buf
    }

    fn make_box_largesize(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (16 + payload.len()) as u64;
        let mut buf = Vec::with_capacity(size as usize);
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_complete() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_data_payload"));
        data.extend_from_slice(&make_box(b"mdat", b"mdat_media_samples"));
        assert!(is_complete_isobmff_media_segment(&data));
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_truncated() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_data"));
        let mdat = make_box(b"mdat", b"mdat_media_samples_long");
        data.extend_from_slice(&mdat[..mdat.len() - 5]);
        assert!(!is_complete_isobmff_media_segment(&data));

        assert!(!is_complete_isobmff_media_segment(&[0, 0, 0]));
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_size_zero() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box_size_zero(b"mdat", b"media_bytes_until_eof"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Final box with size == 0 (extends to EOF) must be supported"
        );
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_size_one_largesize() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box_largesize(b"mdat", b"largesize_payload"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Box with size == 1 (64-bit largesize) must be supported"
        );
    }

    #[test]
    fn test_is_complete_isobmff_illegal_box_sizes_and_malformed() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));

        let mut illegal_size_box = Vec::new();
        illegal_size_box.extend_from_slice(&5u32.to_be_bytes());
        illegal_size_box.extend_from_slice(b"mdat");
        illegal_size_box.extend_from_slice(b"xyz");
        let mut malformed_data = data.clone();
        malformed_data.extend_from_slice(&illegal_size_box);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_data),
            "Illegal box size between 2 and 7 must be safely rejected without panic"
        );

        let mut truncated_largesize = Vec::new();
        truncated_largesize.extend_from_slice(&1u32.to_be_bytes());
        truncated_largesize.extend_from_slice(b"mdat");
        truncated_largesize.extend_from_slice(&[0u8; 4]);
        let mut malformed_largesize = data.clone();
        malformed_largesize.extend_from_slice(&truncated_largesize);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_largesize),
            "Truncated 64-bit largesize header must be rejected without panic"
        );

        let mut small_largesize = Vec::new();
        small_largesize.extend_from_slice(&1u32.to_be_bytes());
        small_largesize.extend_from_slice(b"mdat");
        small_largesize.extend_from_slice(&10u64.to_be_bytes());
        let mut malformed_small_largesize = data.clone();
        malformed_small_largesize.extend_from_slice(&small_largesize);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_small_largesize),
            "Largesize value < 16 must be rejected without panic"
        );
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_padding_boxes() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"free", b"padding1"));
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box(b"skip", b"padding2"));
        data.extend_from_slice(&make_box(b"mdat", b"mdat_payload"));
        data.extend_from_slice(&make_box(b"free", b"padding3"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Padding boxes (free/skip) should be safely skipped"
        );
    }

    #[test]
    fn test_is_complete_isobmff_init_segment() {
        let ftyp = make_box(b"ftyp", b"iso6mp41");
        let moov = make_box(b"moov", b"moov_metadata");

        assert!(!is_complete_isobmff_init_segment(&ftyp));
        assert!(!is_complete_isobmff_init_segment(&moov));

        let mut valid_init = Vec::new();
        valid_init.extend_from_slice(&ftyp);
        valid_init.extend_from_slice(&moov);
        assert!(is_complete_isobmff_init_segment(&valid_init));

        let mut truncated_init = Vec::new();
        truncated_init.extend_from_slice(&ftyp);
        truncated_init.extend_from_slice(&moov[..moov.len() - 3]);
        assert!(!is_complete_isobmff_init_segment(&truncated_init));
    }
}
