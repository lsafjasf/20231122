use serde_json::Value;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn demo_report_preserves_replay_unknown_boundary_and_failure_actions() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "source-ledger-contract-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("demo_project")).unwrap();
    fs::write(
        root.join("demo_project/project.json"),
        include_str!("../demo_project/project.json"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_source-ledger"))
        .arg("demo")
        .env("SOURCE_LEDGER_ROOT", &root)
        .output()
        .expect("run demo binary");
    assert!(
        output.status.success(),
        "demo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value =
        serde_json::from_slice(&fs::read(root.join("artifacts/report.json")).unwrap()).unwrap();
    let sources = report["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 6);

    let replay = &report["replay"];
    assert_eq!(replay["source_id"], "src-recovered");
    assert_eq!(replay["identical"], true);
    assert_eq!(
        replay["original_sha256"], replay["replay_sha256"],
        "replay must bind the same original input"
    );
    assert_eq!(replay["steps"].as_array().unwrap().len(), 3);

    let unknown = sources
        .iter()
        .find(|source| source["spec"]["id"] == "src-unknown")
        .unwrap();
    assert_eq!(unknown["state"], "unknown");
    assert!(unknown["probe"]["sha256"].as_str().unwrap().len() == 64);
    assert!(root
        .join("artifacts/evidence/src-unknown/original.bin")
        .is_file());

    let boundary = sources
        .iter()
        .find(|source| source["spec"]["id"] == "src-boundary")
        .unwrap();
    assert_eq!(boundary["state"], "ambiguous");
    assert!(boundary["probe"]["head_hex"]
        .as_str()
        .unwrap()
        .contains("66 74 79 70"));
    assert!(root
        .join("artifacts/evidence/src-boundary/boundary-window.hex")
        .is_file());

    let manual = report["manual_confirmations"].as_array().unwrap();
    assert!(manual
        .iter()
        .any(|item| { item["source_id"] == "src-unknown" && item["state"] == "pending" }));
    assert!(manual
        .iter()
        .any(|item| { item["source_id"] == "src-boundary" && item["state"] == "pending" }));
    assert!(manual
        .iter()
        .any(|item| { item["source_id"] == "src-recovered" && item["state"] == "confirmed" }));

    let failures = report["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["source_id"], "src-missing");
    assert_eq!(failures[0]["next_actions"].as_array().unwrap().len(), 3);
    for action in failures[0]["next_actions"].as_array().unwrap() {
        assert!(!action["label"].as_str().unwrap().is_empty());
        assert!(!action["detail"].as_str().unwrap().is_empty());
    }

    let markdown = fs::read_to_string(root.join("artifacts/report.md")).unwrap();
    for heading in [
        "## 自动结论",
        "## 人工确认",
        "## 恢复后的重放",
        "## 失败处置",
        "## 证据索引",
    ] {
        assert!(markdown.contains(heading), "missing section {heading}");
    }
    assert!(root.join("artifacts/report.sha256").is_file());
    assert!(root.join("artifacts/runtime/events.jsonl").is_file());

    fs::remove_dir_all(root).unwrap();
}
