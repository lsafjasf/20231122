use queue_priority_corrector::model::DeviceId;
use queue_priority_corrector::transcode;
use std::fs;

#[test]
fn demo_clip_can_be_trimmed_encoded_and_previewed() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.webm");
    transcode::generate_demo_clip(&input, DeviceId::A).unwrap();
    assert!(fs::metadata(&input).unwrap().len() > 1_000);

    let output = dir.path().join("out/trimmed.webm");
    let preview = dir.path().join("out/trimmed.png");
    let result =
        transcode::transcode_segment(&input, &output, Some(&preview), Some((1_000, 2_800)))
            .unwrap();

    let encoded = fs::read(&output).unwrap();
    assert!(encoded.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]));
    assert!(result.output_bytes > 1_000);
    assert_eq!(result.output_sha256.len(), 64);
    assert!(preview.is_file());
    assert!(fs::read(&preview).unwrap().starts_with(b"\x89PNG"));
    assert!(result.pipeline.contains("vp8enc"));
    assert!(result.pipeline.contains("webmmux"));
}
