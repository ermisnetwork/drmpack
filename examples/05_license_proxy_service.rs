use bytes::Bytes;
use drmpack::license::{
    handle_fairplay_certificate, handle_fairplay_license, handle_playready_license,
    handle_widevine_license, LicenseProxy, LicenseResponse,
};
use drmpack::vendor::axinom::AxinomLicenseConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let license_config = AxinomLicenseConfig::from_env().map_err(|e| {
        eprintln!("FATAL: Axinom license configuration error: {e}");
        eprintln!("\nPlease ensure the following environment variables are set in your .env:");
        eprintln!("  AXINOM_WIDEVINE_LICENSE_URL=https://<tenant-id>.drm-widevine-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_FAIRPLAY_LICENSE_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_PLAYREADY_LICENSE_URL=https://<tenant-id>.drm-playready-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_FAIRPLAY_CERT_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/v2/Certificate");
        e
    })?;
    let proxy = LicenseProxy::new(license_config);

    println!("LicenseProxy initialized.");
    println!("  Widevine URL: {}", proxy.config().widevine_license_url);
    println!("  FairPlay URL: {}", proxy.config().fairplay_license_url);
    println!("  PlayReady URL: {}", proxy.config().playready_license_url);
    println!(
        "  FairPlay Cert: {}",
        proxy
            .config()
            .fairplay_cert_url
            .as_deref()
            .unwrap_or("(not configured)")
    );

    let sample_token = std::env::var("AXINOM_ENTITLEMENT_TOKEN")
        .unwrap_or_else(|_| "SAMPLE_OR_TEST_ENTITLEMENT_TOKEN".to_string());

    let cert_result = handle_fairplay_certificate(&proxy, None::<&str>).await;
    match cert_result {
        Ok(cert) => println!(
            "FairPlay Application Certificate loaded: {} bytes",
            cert.len()
        ),
        Err(err) => println!("FairPlay certificate fetch: {err}"),
    }

    let widevine_challenge = Bytes::from_static(b"\x08\x01\x12\x10mock-cdm-challenge");
    let widevine_response =
        handle_widevine_license(&proxy, &widevine_challenge, &sample_token).await;
    report_response("Widevine", widevine_response);

    let fairplay_spc = Bytes::from_static(b"mock-fairplay-spc-payload");
    let fairplay_response = handle_fairplay_license(&proxy, &fairplay_spc, &sample_token).await;
    report_response("FairPlay", fairplay_response);

    let playready_challenge = Bytes::from_static(b"<PlayReadyChallenge>mock</PlayReadyChallenge>");
    let playready_response =
        handle_playready_license(&proxy, &playready_challenge, &sample_token).await;
    report_response("PlayReady", playready_response);

    Ok(())
}

fn report_response(system: &str, result: drmpack::error::Result<LicenseResponse>) {
    match result {
        Ok(res) => {
            println!(
                "{} response: Content-Type: {:?}, payload: {} bytes, axdrm_message: {:?}",
                system,
                res.content_type,
                res.data.len(),
                res.axdrm_message()
            );
        }
        Err(err) => {
            println!("{system} proxy call returned: {err}");
        }
    }
}
