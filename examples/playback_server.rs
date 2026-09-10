mod common;

use common::playback_server::PlaybackServer;
use drmpack::axinom::{generate_axinom_jwt, AxinomKeyConfig, AxinomLicenseConfig};
use drmpack::license::LicenseProxy;
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

fn parse_hex_iv(s: &str) -> Option<[u8; 16]> {
    let clean = s.trim();
    let clean = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
        .unwrap_or(clean)
        .replace('-', "");
    if clean.len() == 32 {
        u128::from_str_radix(&clean, 16).ok().map(u128::to_be_bytes)
    } else {
        None
    }
}

async fn extract_kids_from_dir(dir: &Path) -> Vec<AxinomKeyConfig> {
    let mut configs = Vec::new();
    let mut files_to_check = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];

    while let Some(current_dir) = dirs.pop() {
        if let Ok(mut entries) = tokio::fs::read_dir(current_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "mpd" || e == "m3u8") {
                    files_to_check.push(path);
                }
            }
        }
    }

    let mut seen = HashSet::new();
    for file in files_to_check {
        if let Ok(content) = tokio::fs::read_to_string(&file).await {
            // Extract default_KID from DASH manifests (skipping prefix before first occurrence)
            for part in content.split("default_KID=\"").skip(1) {
                if let Some(kid) = part.split('"').next() {
                    if let Ok(parsed_uuid) = uuid::Uuid::parse_str(kid.trim()) {
                        let kid_str = parsed_uuid.hyphenated().to_string();
                        if seen.insert(kid_str.clone()) {
                            configs.push(AxinomKeyConfig::new(kid_str));
                        }
                    }
                }
            }
            // Extract skd:// URI from HLS playlists (skipping prefix before first occurrence)
            for part in content.split("URI=\"skd://").skip(1) {
                if let Some(raw_skd) = part.split('"').next() {
                    let clean = raw_skd.trim();
                    let (kid_raw, iv_opt) = if let Some((k, iv_str)) = clean.split_once(':') {
                        (k.trim(), parse_hex_iv(iv_str))
                    } else {
                        (clean, None)
                    };
                    if let Ok(parsed_uuid) = uuid::Uuid::parse_str(kid_raw) {
                        let kid_str = parsed_uuid.hyphenated().to_string();
                        let iv = iv_opt.unwrap_or_else(|| *parsed_uuid.as_bytes());
                        if let Some(existing) = configs.iter_mut().find(|c| c.kid == kid_str) {
                            existing.iv = Some(iv);
                        } else if seen.insert(kid_str.clone()) {
                            configs.push(AxinomKeyConfig::new(kid_str).with_iv(iv));
                        }
                    }
                }
            }
        }
    }

    configs
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = dotenvy::dotenv();

    let args: Vec<String> = env::args().collect();
    let port = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse::<u16>().ok())
        .unwrap_or(8080);
    let cdn_dir = args
        .windows(2)
        .find(|w| w[0] == "--cdn-dir")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("scratch/cdn_storage"));

    if !cdn_dir.exists() {
        tokio::fs::create_dir_all(&cdn_dir).await?;
    }

    let is_dual = args.iter().any(|a| a == "--dual") || cdn_dir.join("cenc").exists();

    let manifest_url = if is_dual {
        format!("http://127.0.0.1:{port}/cenc/live.mpd")
    } else {
        format!("http://127.0.0.1:{port}/live.mpd")
    };
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;

    let mut server = PlaybackServer::new(cdn_dir.clone(), manifest_url);
    if is_dual {
        server = server.with_drm_scheme("dual");
    }

    // Auto-detect Axinom credentials and discovered KIDs
    let com_key_id = env::var("AXINOM_COMMUNICATION_KEY_ID").ok();
    let com_key = env::var("AXINOM_COMMUNICATION_KEY").ok();
    let kids = extract_kids_from_dir(&cdn_dir).await;
    if !kids.is_empty() {
        let kids_strings: Vec<String> = kids.iter().map(|k| k.kid.clone()).collect();
        server = server.with_kids(kids_strings);
    }

    if let (Some(com_key_id), Some(com_key)) = (com_key_id, com_key) {
        if !com_key_id.is_empty() && !com_key.is_empty() {
            if kids.is_empty() {
                eprintln!(
                    "Warning: No Key IDs (KIDs) discovered in {}. Run packaging first to produce encrypted streams.",
                    cdn_dir.display()
                );
            } else {
                let token = generate_axinom_jwt(&com_key_id, &com_key, &kids)?;
                let license_config = AxinomLicenseConfig::from_env().map_err(|e| {
                    eprintln!("FATAL: Axinom communication keys configured but license endpoints missing: {e}");
                    eprintln!("\nPlease ensure the following environment variables are set in your .env:");
                    eprintln!("  AXINOM_WIDEVINE_LICENSE_URL=https://<tenant-id>.drm-widevine-licensing.axprod.net/AcquireLicense");
                    eprintln!("  AXINOM_FAIRPLAY_LICENSE_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/AcquireLicense");
                    eprintln!("  AXINOM_PLAYREADY_LICENSE_URL=https://<tenant-id>.drm-playready-licensing.axprod.net/AcquireLicense");
                    eprintln!("  AXINOM_FAIRPLAY_CERT_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/v2/Certificate");
                    e
                })?;
                let license_proxy = LicenseProxy::new(license_config);

                if is_dual {
                    let _ = license_proxy.preload_fairplay_certificate().await;
                }

                let scheme = if is_dual { "dual" } else { "widevine" };
                server = server
                    .with_drm_scheme(scheme)
                    .with_license_proxy(license_proxy, token);

                println!(
                    "Axinom DRM LicenseProxy enabled ({} Key IDs discovered in manifests).",
                    kids.len()
                );
            }
        }
    }

    // Auto-detect ClearKey parameters
    if let Ok(clearkey_var) = env::var("CLEARKEY_KEYS") {
        for pair in clearkey_var.split(',') {
            let parts: Vec<&str> = pair.split(':').collect();
            if parts.len() == 2 {
                server = server
                    .with_clearkey(parts[0], parts[1])
                    .with_drm_scheme("clearkey");
            }
        }
    }

    println!("====================================================");
    println!("drmpack CDN Mock & Playback Player HTTP Server");
    println!("====================================================");
    println!("Serving CDN storage: {}", cdn_dir.display());
    println!("Player Web UI:       http://127.0.0.1:{port}/");
    if is_dual {
        println!("Dual CENC DASH:      http://127.0.0.1:{port}/cenc/live.mpd");
        println!("Dual CBCS DASH:      http://127.0.0.1:{port}/cbcs/live.mpd");
        println!("Dual CBCS HLS:       http://127.0.0.1:{port}/cbcs/live.m3u8");
        println!("Dual CENC HLS:       http://127.0.0.1:{port}/cenc/live.m3u8");
    } else {
        println!("Live DASH Manifest:  http://127.0.0.1:{port}/live.mpd");
        println!("Live HLS Manifest:   http://127.0.0.1:{port}/live.m3u8");
    }
    println!("Press Ctrl+C to stop playback server.");
    println!("====================================================");

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            println!("\nShutdown signal received (Ctrl+C). Stopping HTTP server...");
            let _ = shutdown_tx.send(());
        }
    });

    server.run(listener, shutdown_rx).await;

    println!("Playback server shut down cleanly.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_iv() {
        let expected = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        // Standard 32 hex chars
        assert_eq!(
            parse_hex_iv("000102030405060708090a0b0c0d0e0f"),
            Some(expected)
        );
        // With 0x prefix
        assert_eq!(
            parse_hex_iv("0x000102030405060708090a0b0c0d0e0f"),
            Some(expected)
        );
        // With 0X prefix
        assert_eq!(
            parse_hex_iv("0X000102030405060708090a0b0c0d0e0f"),
            Some(expected)
        );
        // With hyphens (UUID-formatted IV)
        assert_eq!(
            parse_hex_iv("00010203-0405-0607-0809-0a0b0c0d0e0f"),
            Some(expected)
        );
        // Uppercase
        assert_eq!(
            parse_hex_iv("000102030405060708090A0B0C0D0E0F"),
            Some(expected)
        );
        // Invalid length
        assert_eq!(parse_hex_iv("00010203"), None);
        // Invalid characters
        assert_eq!(parse_hex_iv("000102030405060708090a0b0c0d0e0g"), None);
        // Empty
        assert_eq!(parse_hex_iv(""), None);
    }

    #[tokio::test]
    async fn test_extract_kids_from_dir_dash_and_hls() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_playback_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let dash_mpd = r#"<?xml version="1.0"?>
<MPD>
  <Period>
    <AdaptationSet>
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc" cenc:default_KID="11111111-1111-1111-1111-111111111111"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let hls_m3u8 = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1",URI="skd://22222222-2222-2222-2222-222222222222:0x000102030405060708090a0b0c0d0e0f"
#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1",URI="skd://33333333-3333-3333-3333-333333333333"
"#;

        tokio::fs::write(temp_dir.join("live.mpd"), dash_mpd)
            .await
            .unwrap();
        tokio::fs::write(temp_dir.join("live.m3u8"), hls_m3u8)
            .await
            .unwrap();

        let configs = extract_kids_from_dir(&temp_dir).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        assert_eq!(configs.len(), 3);

        // DASH CENC key should have no IV
        let dash_key = configs
            .iter()
            .find(|c| c.kid == "11111111-1111-1111-1111-111111111111")
            .expect("DASH key missing");
        assert_eq!(dash_key.iv, None);

        // HLS key with explicit IV
        let hls_key1 = configs
            .iter()
            .find(|c| c.kid == "22222222-2222-2222-2222-222222222222")
            .expect("HLS key 1 missing");
        assert_eq!(
            hls_key1.iv,
            Some([
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f
            ])
        );

        // HLS key without explicit IV falls back to parsed UUID bytes
        let hls_key2 = configs
            .iter()
            .find(|c| c.kid == "33333333-3333-3333-3333-333333333333")
            .expect("HLS key 2 missing");
        let expected_uuid = uuid::Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        assert_eq!(hls_key2.iv, Some(*expected_uuid.as_bytes()));
    }
}
