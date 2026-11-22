use proxyrecover::journal::read_journal;
use proxyrecover::{experiment, replay, report};
use std::io::Write;

#[test]
fn experiment_is_replayable_and_evidence_is_tamper_evident() {
    let temp = tempfile::tempdir().expect("temp dir");
    let out_dir = temp.path().join("latest");
    let mut progress = |_message: &str| {};
    let output = experiment::run_experiment(&out_dir, &mut progress).expect("experiment runs");
    assert!(
        output.all_passed,
        "automated checks failed: {:#?}",
        output.checks
    );

    let records = read_journal(&output.journal_path).expect("journal parses");
    assert!(
        records.len() >= 23,
        "unexpected journal length {}",
        records.len()
    );
    assert!(records.iter().all(|record| {
        !record.purpose.is_empty()
            && !record.input.is_null()
            && !record.result.is_null()
            && !record.state_before.is_empty()
            && !record.state_after.is_empty()
    }));

    let invalid_a = records
        .iter()
        .find(|record| record.step == "invalidate-clip-a-initial")
        .expect("invalidation step");
    assert_eq!(invalid_a.result["hash_changed"], true);
    let control = records
        .iter()
        .find(|record| record.step == "unknown-extension-control")
        .expect("control step");
    assert_eq!(control.result["accepted"], false);
    assert_eq!(control.result["proxy_created"], false);
    assert_eq!(control.result["extension"], "weird");

    let replay = replay::replay_journal(&output.journal_path, &mut progress).expect("replay runs");
    assert_eq!(replay.matched, replay.total);
    assert!(
        replay.mismatches.is_empty(),
        "replay mismatches: {:#?}",
        replay.mismatches
    );

    let verified = report::verify_manifest(&output.manifest_path).expect("manifest verifies");
    assert!(verified >= 30, "manifest too small: {verified}");

    let victim = out_dir.join("evidence/proxy-clip-a-initial.png");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&victim)
        .expect("open evidence");
    file.write_all(b"tamper").expect("tamper evidence");
    assert!(
        report::verify_manifest(&output.manifest_path).is_err(),
        "tampered evidence must fail verification"
    );
}
