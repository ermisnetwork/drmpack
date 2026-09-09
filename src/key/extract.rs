//! Extract DRM key information from packaged content on disk.
//!
//! For standalone playback servers that don't have a `PackagingSession` —
//! only packaged files on disk. Parses MPD manifests to extract KIDs and
//! encryption scheme info, returning a `KeySet` ready for JWT generation.

use crate::key::{ContentKey, KeyID, KeySet};
use crate::types::{EncryptionScheme, QualityTier, TrackType};
use std::collections::HashSet;
use std::path::Path;

/// Extract KIDs and encryption scheme info from MPD files in a directory.
///
/// Handles dual mode (`cenc/` + `cbcs/` subdirs) automatically.
/// Returns a `KeySet` with correct `encryption_scheme` and IV derived for
/// CBCS keys (IV = KID bytes, FairPlay convention).
///
/// # Example
/// ```no_run
/// let key_set = drmpack::key::extract_keys_from_dir("scratch/example08_live");
/// // key_set is ready for JWT generation:
/// // let jwt = key_set.generate_axinom_jwt(&com_key_id, &com_key)?;
/// ```
pub fn extract_keys_from_dir(dir: impl AsRef<Path>) -> KeySet {
    let dir = dir.as_ref();
    let mut key_set = KeySet::new();
    let mut seen = HashSet::new();

    for (subdir, is_cbcs) in [("cenc", false), ("cbcs", true), ("", false)] {
        let search_dir = dir.join(subdir);
        if !search_dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&search_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("mpd") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            // Detect scheme from subdir name or MPD value="cbcs" attribute
            let mpd_is_cbcs = is_cbcs || content.contains("value=\"cbcs\"");
            let scheme = if mpd_is_cbcs {
                EncryptionScheme::Cbcs
            } else {
                EncryptionScheme::Cenc
            };

            // Extract default_KID="uuid" from ContentProtection elements
            for segment in content.split("default_KID=\"") {
                if let Some(end) = segment.find('"') {
                    let kid_str = &segment[..end];
                    if kid_str.len() >= 32 && seen.insert((kid_str.to_string(), scheme)) {
                        if let Ok(uuid) = uuid::Uuid::parse_str(kid_str) {
                            let kid = KeyID::new(uuid);
                            // ponytail: key bytes are unknown from disk, use zeroed placeholder.
                            // This KeySet is only used for JWT generation (KID + IV), not encryption.
                            let mut ck = ContentKey::new_with_scheme(
                                kid,
                                [0u8; 16],
                                QualityTier::hd(),
                                TrackType::Video,
                                scheme,
                            );
                            if mpd_is_cbcs {
                                // FairPlay convention: IV = KID bytes
                                ck = ck.with_iv(*uuid.as_bytes());
                            }
                            key_set.insert_key(ck);
                        }
                    }
                }
            }
        }
    }
    key_set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("drmpack_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_extract_keys_from_dual_dir() {
        let tmp = test_dir();
        let cenc_dir = tmp.join("cenc");
        let cbcs_dir = tmp.join("cbcs");
        fs::create_dir_all(&cenc_dir).unwrap();
        fs::create_dir_all(&cbcs_dir).unwrap();

        fs::write(
            cenc_dir.join("live.mpd"),
            r#"<ContentProtection value="cenc" cenc:default_KID="d1e86b3b-be5c-4490-ab2e-eb2a335cfa51"/>"#,
        )
        .unwrap();
        fs::write(
            cbcs_dir.join("live.mpd"),
            r#"<ContentProtection value="cbcs" cenc:default_KID="99db4516-7404-4a2f-a2c0-ceef767c8788"/>"#,
        )
        .unwrap();

        let key_set = extract_keys_from_dir(&tmp);
        assert_eq!(key_set.len(), 2);

        // CENC key should not have IV
        let cenc_key = key_set
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .unwrap();
        assert!(cenc_key.iv.is_none());

        // CBCS key should have IV = KID bytes
        let cbcs_key = key_set
            .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
            .unwrap();
        assert!(cbcs_key.iv.is_some());
        let expected_iv = *uuid::Uuid::parse_str("99db4516-7404-4a2f-a2c0-ceef767c8788")
            .unwrap()
            .as_bytes();
        assert_eq!(cbcs_key.iv.unwrap(), expected_iv);

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_extract_keys_empty_dir() {
        let tmp = test_dir();
        let key_set = extract_keys_from_dir(&tmp);
        assert!(key_set.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }
}
