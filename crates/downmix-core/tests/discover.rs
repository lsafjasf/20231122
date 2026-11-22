use downmix_core::discover::discover_objects;
use downmix_core::model::DetectionKind;
use std::fs;
use std::path::Path;

fn write_wav_like(path: &Path) {
    let mut bytes = vec![0u8; 64];
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[8..12].copy_from_slice(b"WAVE");
    fs::write(path, bytes).unwrap();
}

#[test]
fn sniffs_unknown_extension_and_skips_non_audio() {
    let root = tempfile::tempdir().unwrap();
    write_wav_like(&root.path().join("mystery.object"));
    write_wav_like(&root.path().join("noextension"));
    write_wav_like(&root.path().join("known.wav"));
    fs::write(root.path().join("notes.txt"), b"hello world").unwrap();
    fs::create_dir_all(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/random.bin"), vec![0u8; 32]).unwrap();

    let objects = discover_objects(root.path()).unwrap();
    let by_name: std::collections::HashMap<_, _> = objects
        .iter()
        .map(|o| (o.path.file_name().unwrap().to_string_lossy().into_owned(), o))
        .collect();

    assert_eq!(objects.len(), 3);
    assert_eq!(
        by_name["mystery.object"].detection,
        DetectionKind::SniffedContent
    );
    assert_eq!(
        by_name["noextension"].detection,
        DetectionKind::SniffedContent
    );
    assert_eq!(
        by_name["known.wav"].detection,
        DetectionKind::KnownExtension
    );
    assert!(by_name.get("notes.txt").is_none());
    assert!(by_name.get("random.bin").is_none());
    for object in &objects {
        assert_eq!(object.sha256.len(), 64);
    }
}
