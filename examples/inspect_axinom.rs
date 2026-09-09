use base64::prelude::*;
use drmpack::axinom::AxinomConfig;
use drmpack::cpix::builder::CpixRequestBuilder;
use drmpack::cpix::parser::CpixResponseParser;
use drmpack::key::KeyRequest;
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, TrackType};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("============================================================");
    println!("       AXINOM KEY SERVICE - RAW XML INSPECTOR TOOL          ");
    println!("============================================================\n");

    let args: Vec<String> = env::args().collect();
    let override_key_ids = args.iter().any(|a| a == "--override-key-ids");
    let is_multi_tier = args.iter().any(|a| a == "--multi-tier");
    let scheme_str = args
        .windows(2)
        .find(|w| w[0] == "--scheme")
        .map(|w| w[1].as_str())
        .unwrap_or("dual");

    let scheme = match scheme_str.to_lowercase().as_str() {
        "cenc" => EncryptionScheme::Cenc,
        "cbcs" => EncryptionScheme::Cbcs,
        "dual" => EncryptionScheme::Dual,
        other => {
            eprintln!("Unknown scheme '{other}', defaulting to Dual");
            EncryptionScheme::Dual
        }
    };

    let config = match AxinomConfig::from_env() {
        Ok(c) => c.with_override_key_ids(override_key_ids),
        Err(e) => {
            eprintln!("Error loading Axinom credentials: {e}");
            eprintln!(
                "Please make sure AXINOM_TENANT_ID, AXINOM_MANAGEMENT_KEY, and AXINOM_ENDPOINT (or AXINOM_SPEKE_ENDPOINT) are set in .env"
            );
            return Ok(());
        }
    };

    println!("Configuration:");
    println!("  - Endpoint: {}", config.endpoint);
    println!("  - Tenant ID: {}", config.tenant_id);
    println!("  - Management Key: [REDACTED]");
    println!("  - Override Key IDs: {}", config.override_key_ids);
    println!("  - Selected Scheme: {scheme:?}");
    println!("  - Multi-tier Mode: {is_multi_tier}\n");

    let content_id = format!("inspect-{}", uuid::Uuid::new_v4());
    let mut req = KeyRequest::new(&content_id);

    if is_multi_tier {
        req = req
            .with_tier(TrackType::Video, QualityTier::uhd_4k())
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_tier(TrackType::Video, QualityTier::sd())
            .with_tier(TrackType::Audio, QualityTier::sd());
    } else {
        req = req.with_tier(TrackType::Video, QualityTier::hd());
    }

    req = req
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_drm_system(DrmSystem::PlayReady);

    let concrete_schemes = match scheme {
        EncryptionScheme::Dual => vec![EncryptionScheme::Cenc, EncryptionScheme::Cbcs],
        single => vec![single],
    };

    let client = reqwest::Client::new();

    for cur_scheme in concrete_schemes {
        let mut sub_req = req.clone();
        sub_req.encryption_schemes = vec![cur_scheme];

        println!("------------------------------------------------------------");
        println!(">>> EXECUTING REQUEST FOR SCHEME: {cur_scheme:?}");
        println!("------------------------------------------------------------\n");

        let (xml_request, specs) = CpixRequestBuilder::build_with_specs(&sub_req)?;

        println!("[1] OUTGOING CPIX 2.3 XML REQUEST:");
        println!("{xml_request}\n");

        let credentials = format!("{}:{}", config.tenant_id, config.management_key);
        let auth_value = format!("Basic {}", BASE64_STANDARD.encode(credentials.as_bytes()));

        let mut http_req = client
            .post(&config.endpoint)
            .header(reqwest::header::AUTHORIZATION, auth_value)
            .header("X-Speke-Version", "2.0")
            .header(
                reqwest::header::USER_AGENT,
                concat!("drmpack-inspector/", env!("CARGO_PKG_VERSION")),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .header(reqwest::header::ACCEPT, "application/xml")
            .body(xml_request);

        if config.override_key_ids {
            http_req = http_req.query(&[("overrideKeyIds", "true")]);
        }

        println!("[2] SENDING HTTP POST TO AXINOM...");
        let resp = http_req.send().await?;
        let status = resp.status();
        let headers = resp.headers().clone();

        println!("[3] RECEIVED HTTP RESPONSE:");
        println!("  - Status: {status}");
        if let Some(err_msg) = headers.get("x-axdrm-errormessage") {
            println!(
                "  - X-AxDRM-ErrorMessage: {:?}",
                err_msg.to_str().unwrap_or("")
            );
        }

        let resp_body = resp.text().await?;
        println!("\n[4] RAW CPIX 2.3 XML RESPONSE FROM AXINOM:");
        println!("{resp_body}\n");

        if status.is_success() {
            println!("[5] PARSED KEYSET SUMMARY:");
            let key_set = CpixResponseParser::parse(&resp_body, Some(&specs))?;
            println!("  Total Keys Acquired: {}", key_set.len());
            for key in key_set.all_keys() {
                println!(
                    "  - KID: {} | Scheme: {:?} | Tier: {:?} | Track: {:?} | Key: {:032x}",
                    key.kid.0.hyphenated(),
                    key.encryption_scheme,
                    key.quality_tier.0.as_str(),
                    key.track_type,
                    u128::from_be_bytes(key.key)
                );
            }
            println!("  DRM Signaling / PSSH entries: {}", key_set.pssh.len());
            for pssh in &key_set.pssh {
                println!(
                    "    * DRM System: {:?} | Data Len: {} bytes",
                    pssh.drm_system,
                    pssh.data.len()
                );
            }
        } else {
            eprintln!("Request failed with HTTP {status}!");
        }
        println!();
    }

    println!("============================================================");
    println!("                     INSPECTION COMPLETE                    ");
    println!("============================================================\n");

    Ok(())
}
