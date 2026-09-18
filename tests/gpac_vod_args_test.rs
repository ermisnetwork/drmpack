use drmpack::gpac::vod::GpacVodProcessConfig;
use drmpack::vod::{VodInputSource, VodMode};
use std::path::PathBuf;

#[test]
fn test_gpac_vod_single_file_args() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/vod"),
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_manifest_name("vod");

    let args = config.build_args();

    assert!(args.iter().any(|a| a.starts_with("-logs=")));
    assert!(args.contains(&"-p=0".to_string()));
    assert!(args.contains(&"-threads=1".to_string()));
    assert!(args.contains(&"-i".to_string()));
    // Input must include representation mapping
    let input_arg = args
        .iter()
        .find(|a| a.starts_with("/inputs/movie.mp4"))
        .unwrap();
    assert!(input_arg.contains("#Representation="));
    assert!(input_arg.contains("#HLSPL="));

    // Cecrypt filter
    assert!(args.contains(&"cecrypt:cfile=/keys/drm.xml".to_string()));

    // Output dasher options
    assert!(args.contains(&"-o".to_string()));
    let dasher_arg = args
        .iter()
        .find(|a| a.starts_with("/out/vod/vod.mpd:"))
        .unwrap();
    assert!(dasher_arg.contains(":dual:"));
    assert!(dasher_arg.contains(":segdur=2:"));
    assert!(dasher_arg.contains(":profile=onDemand:"));
    assert!(dasher_arg.contains(":pssh=mv:"));
    assert!(dasher_arg.contains(":template=$RepresentationID$"));
}

#[test]
fn test_gpac_vod_segmented_args() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/segmented"),
    )
    .with_vod_mode(VodMode::Segmented)
    .with_segment_duration(4.0)
    .with_manifest_name("index");

    let args = config.build_args();
    let dasher_arg = args
        .iter()
        .find(|a| a.starts_with("/out/segmented/index.mpd:"))
        .unwrap();
    assert!(dasher_arg.contains(":dual:"));
    assert!(dasher_arg.contains(":segdur=4:"));
    assert!(dasher_arg.contains(":template=$RepresentationID$_$Init=init$$Number$"));
    assert!(!dasher_arg.contains(":profile=onDemand:"));
}

#[test]
fn test_gpac_vod_track_files_input_args() {
    let input = VodInputSource::TrackFiles(vec![
        PathBuf::from("/inputs/video.mp4"),
        PathBuf::from("/inputs/audio.mp4"),
    ]);
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/vod"),
    )
    .with_gpac_bin("/opt/bin/gpac");

    assert_eq!(config.gpac_bin, "/opt/bin/gpac");
    let args = config.build_args();
    let i_count = args.iter().filter(|a| a.as_str() == "-i").count();
    assert_eq!(i_count, 2, "Must pass -i for each track file");
    assert!(args
        .iter()
        .any(|a| a.contains("/inputs/video.mp4:#Representation=")));
    assert!(args
        .iter()
        .any(|a| a.contains("/inputs/audio.mp4:#Representation=")));
}

#[test]
fn test_gpac_vod_args_includes_isolation_and_threads() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/vod"),
    )
    .with_threads(2)
    .with_temp_dir(PathBuf::from("/tmp/session_tmp"));

    let args = config.build_args();

    // Must disable configuration writing
    assert!(args.contains(&"-p=0".to_string()));
    // Must set thread count
    assert!(args.contains(&"-threads=2".to_string()));
    // Must isolate temp directory
    assert!(args.contains(&"-tmp=/tmp/session_tmp".to_string()));
}
