fn main() {
    let config = audit_core::scenario::default_config();
    let samples = audit_core::scenario::fixture_samples();
    // replicate headless::assemble without depending on the binary crate
    let run = audit_core::run(&samples, &config);
    let report = audit_core::build_report(&run);
    let trunc = audit_core::replay::capture_truncation(
        "caps 变更触发 flush", &samples, run.records[..3].to_vec(), &config);
    let replay = audit_core::replay::recover_and_replay(&trunc);
    let out = serde_json::json!({
        "config": config, "run": run, "report": report,
        "truncation": trunc, "replay": replay,
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hdr_audit_evidence.json");
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    println!("wrote {}", path.display());
}
