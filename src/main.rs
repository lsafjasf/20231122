use queue_priority_corrector::export::{export_bundle, verify_bundle};
use queue_priority_corrector::model::Session;
use queue_priority_corrector::store::SessionStore;
use queue_priority_corrector::{demo, transcode, ui};
use std::path::PathBuf;

fn main() -> gtk4::glib::ExitCode {
    match run() {
        Ok(code) => gtk4::glib::ExitCode::from(code),
        Err(error) => {
            eprintln!("错误：{error:#}");
            gtk4::glib::ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<u8> {
    let mut session = None;
    let mut export = None;
    let mut verify = None;
    let mut replay = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(0);
            }
            "--session" => {
                session = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--session 缺少路径"))?,
                ));
            }
            "--export" => {
                export = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--export 缺少目录"))?,
                ));
            }
            "--verify" => {
                verify = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--verify 缺少目录"))?,
                ));
            }
            "--replay-run" => {
                replay = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--replay-run 缺少 run ID"))?,
                );
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--output 缺少路径"))?,
                ));
            }
            value if value.starts_with("--session=") => {
                session = Some(PathBuf::from(&value["--session=".len()..]));
            }
            value if value.starts_with("--export=") => {
                export = Some(PathBuf::from(&value["--export=".len()..]));
            }
            value if value.starts_with("--verify=") => {
                verify = Some(PathBuf::from(&value["--verify=".len()..]));
            }
            value if value.starts_with("--replay-run=") => {
                replay = Some(value["--replay-run=".len()..].to_owned());
            }
            value if value.starts_with("--output=") => {
                output = Some(PathBuf::from(&value["--output=".len()..]));
            }
            other => anyhow::bail!("未知参数：{other}（使用 --help 查看用法）"),
        }
    }

    if let Some(directory) = verify {
        let report = verify_bundle(&directory)?;
        println!(
            "复核包：{}\n已校验：{} 个文件\n错误：{}",
            report.directory.display(),
            report.checked,
            report.errors.len()
        );
        for error in &report.errors {
            eprintln!("- {error}");
        }
        return Ok(if report.is_valid() { 0 } else { 2 });
    }

    if let Some(directory) = export {
        transcode::ensure_initialized()?;
        let session_path = session.unwrap_or_else(|| PathBuf::from("artifacts/queue_session.json"));
        let store = SessionStore::new(session_path);
        let mut snapshot = load_or_seed(&store)?;
        snapshot.append_evidence(
            queue_priority_corrector::model::EvidenceKind::ExportCreated,
            None,
            None,
            format!("命令行导出复核包：{}", directory.display()),
            vec![
                queue_priority_corrector::model::field("export_dir", directory.display()),
                queue_priority_corrector::model::field("scope", "entire_session"),
            ],
        );
        let bundle = export_bundle(&snapshot, &directory)?;
        eprintln!(
            "已导出 {} 个证据文件和 {} 个媒体副本到 {}",
            bundle.file_count,
            bundle.media_count,
            bundle.directory.display()
        );
        return Ok(0);
    }

    if let Some(run_id) = replay {
        transcode::ensure_initialized()?;
        let session_path = session
            .ok_or_else(|| anyhow::anyhow!("--replay-run 必须同时指定 --session <会话.json>"))?;
        let store = SessionStore::new(session_path);
        let loaded = store
            .load()?
            .ok_or_else(|| anyhow::anyhow!("会话不存在：{}", store.path().display()))?;
        let session = loaded.session;
        let run = session
            .runs
            .iter()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| anyhow::anyhow!("会话中找不到运行记录：{run_id}"))?;
        let segment = session
            .segment(&run.segment_id)
            .ok_or_else(|| anyhow::anyhow!("运行记录对应的片段不存在：{}", run.segment_id))?;
        let output = output.unwrap_or_else(|| {
            store
                .path()
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("replays")
                .join(format!("{run_id}.webm"))
        });
        let result = transcode::transcode_segment(
            std::path::Path::new(&run.input_path),
            &output,
            None,
            Some(if run.out_ms > run.in_ms {
                (run.in_ms, run.out_ms)
            } else {
                (segment.in_ms, segment.out_ms)
            }),
        )?;
        println!(
            "重放完成：{}\n输出 SHA-256：{}\n记录 SHA-256：{}\n管线：{}",
            output.display(),
            result.output_sha256,
            run.output_sha256.as_deref().unwrap_or("—"),
            result.pipeline
        );
        return Ok(0);
    }

    ui::run(session);
    Ok(0)
}

fn load_or_seed(store: &SessionStore) -> anyhow::Result<Session> {
    if let Some(loaded) = store.load()? {
        return Ok(loaded.session);
    }
    let mut session = demo::seed_session(
        store
            .path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    );
    demo::ensure_demo_media(&mut session)?;
    store.save(&session)?;
    Ok(session)
}

fn print_help() {
    println!(
        "转码队列 · 局部修正\n\n\
         用法：\n\
           queue-priority-corrector [--session <会话.json>]\n\
           queue-priority-corrector --session <会话.json> --export <复核包目录>\n\
           queue-priority-corrector --verify <复核包目录>\n\n\
           queue-priority-corrector --session <会话.json> --replay-run <run-id> [--output <文件>]\n\n\
         默认会话：artifacts/queue_session.json（或环境变量 QP_SESSION）。\n\
         --export 与 --verify 不打开图形界面，供批处理和同事复核使用。"
    );
}
