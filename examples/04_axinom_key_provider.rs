use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::Rendition;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let output_dir = std::env::temp_dir().join(format!("drmpack_example_04_{}", Uuid::new_v4()));

    let axinom_config = AxinomConfig::from_env().map_err(|e| {
        eprintln!("FATAL: Axinom configuration error: {e}");
        eprintln!(
            "Please configure AXINOM_TENANT_ID, AXINOM_MANAGEMENT_KEY, and AXINOM_ENDPOINT in .env"
        );
        eprintln!(
            "(Note: To run offline packaging without cloud credentials, run: `cargo run --example 01_basic_live_cenc`)"
        );
        e
    })?;

    let provider = AxinomProvider::new(axinom_config);

    let rendition = Rendition::video_hd();

    let session_config = PackagingSessionConfig::cenc("axinom-live-04")
        .with_rendition(rendition)
        .with_output_dir(&output_dir);

    match PackagingSession::create(session_config, &provider).await {
        Ok(mut session) => {
            let media_data = get_media_data().await?;
            session.push(media_data).await?;
            session.check_status().await?;
            session.close().await?;

            println!("Axinom session packaging completed successfully.");
            println!("HLS Manifest:  {}", session.hls_manifest_path()?.display());
            println!("DASH Manifest: {}", session.dash_manifest_path()?.display());

            let _ = tokio::fs::remove_dir_all(&output_dir).await;
        }
        Err(err) => {
            println!("PackagingSession creation returned error: {err}");
            println!("To fetch live keys from Axinom, configure valid credentials in .env");
        }
    }

    Ok(())
}

async fn get_media_data() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && !args[1].starts_with('-') {
        return Ok(tokio::fs::read(&args[1]).await?);
    }

    let temp_file = std::env::temp_dir().join(format!("test_axinom_{}.mp4", Uuid::new_v4()));
    let output = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=1280x720:rate=30",
            "-c:v",
            "libx264",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            temp_file.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    let bytes = tokio::fs::read(&temp_file).await?;
    let _ = tokio::fs::remove_file(&temp_file).await;
    Ok(bytes)
}
