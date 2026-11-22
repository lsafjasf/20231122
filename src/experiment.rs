use crate::gst;
use crate::hash;
use crate::journal::{read_journal, Journal};
use crate::model::{
    extension_of, extension_supported, AssetMeta, Check, MediaFingerprint, ProxySpec, ProxyState,
    ProxyValidation, UseOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

pub const ASSET_A: &str = "clip-a";
pub const ASSET_B: &str = "clip-b";
pub const CONTROL: &str = "control-unknown";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioConfig {
    pub schema_version: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub proxy_width: u32,
    pub proxy_height: u32,
    pub pattern_a: String,
    pub pattern_b: String,
    pub unknown_extension: String,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            source_width: 640,
            source_height: 360,
            proxy_width: 320,
            proxy_height: 180,
            pattern_a: "smpte".into(),
            pattern_b: "ball".into(),
            unknown_extension: "weird".into(),
        }
    }
}

impl ScenarioConfig {
    pub fn proxy_spec(&self) -> ProxySpec {
        ProxySpec {
            width: self.proxy_width,
            height: self.proxy_height,
            pixel_format: "RGB".into(),
            encoder: "pngenc".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunOutput {
    pub journal_path: PathBuf,
    pub report_path: PathBuf,
    pub manifest_path: PathBuf,
    pub checks: Vec<Check>,
    pub all_passed: bool,
}

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn source(&self, id: &str, ext: &str) -> PathBuf {
        self.root
            .join("workspace/sources")
            .join(format!("{id}.{ext}"))
    }

    fn proxy(&self, id: &str) -> PathBuf {
        self.root
            .join("workspace/proxies")
            .join(format!("{id}.proxy.png"))
    }

    fn meta(&self, id: &str) -> PathBuf {
        self.root
            .join("workspace/metadata")
            .join(format!("{id}.json"))
    }

    fn evidence(&self, name: &str) -> PathBuf {
        self.root.join("evidence").join(name)
    }

    fn relative(&self, path: &Path) -> String {
        hash::relative_string(&self.root, path)
    }

    fn load(&self, id: &str) -> Result<AssetMeta, String> {
        let path = self.meta(id);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取元数据 {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("解析元数据 {}: {e}", path.display()))
    }

    fn save(&self, meta: &AssetMeta) -> Result<(), String> {
        let mut snapshot = meta.clone();
        snapshot.updated_at_unix_ms = hash::now_unix_ms();
        hash::write_json_atomic(&self.meta(&meta.asset_id), &snapshot)
    }
}

fn prepare_out_dir(out_dir: &Path) -> Result<(), String> {
    if out_dir.as_os_str().is_empty() || out_dir == Path::new("/") {
        return Err("拒绝清空空路径或根目录".into());
    }
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)
            .map_err(|e| format!("清理输出目录 {}: {e}", out_dir.display()))?;
    }
    for dir in [
        "workspace/sources",
        "workspace/proxies",
        "workspace/metadata",
        "evidence",
    ] {
        std::fs::create_dir_all(out_dir.join(dir)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn inspect_candidate(
    path: &Path,
    spec: &ProxySpec,
) -> Result<(u64, String, MediaFingerprint), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取候选代理 {}: {e}", path.display()))?
        .len();
    let sha = hash::sha256_file(path)?;
    let fingerprint = gst::fingerprint_media(path, spec)?;
    Ok((size, sha, fingerprint))
}

fn validate_existing(meta: &AssetMeta) -> ProxyValidation {
    let path = Path::new(&meta.proxy_path);
    let exists = path.is_file();
    let actual_size = std::fs::metadata(path).ok().map(|m| m.len());
    let actual_sha256 = if exists {
        hash::sha256_file(path).ok()
    } else {
        None
    };
    let size_matches = actual_size.is_some() && actual_size == meta.proxy_size;
    let hash_matches = actual_sha256.is_some() && actual_sha256 == meta.proxy_sha256;
    let (decodes, fingerprint, dimensions_match) = if exists && hash_matches {
        match gst::fingerprint_media(path, &meta.proxy_spec) {
            Ok(fp) => {
                let dims = fp.width == meta.proxy_spec.width && fp.height == meta.proxy_spec.height;
                (true, Some(fp), dims)
            }
            Err(_) => (false, None, false),
        }
    } else {
        (false, None, false)
    };
    let mut reasons = Vec::new();
    if !exists {
        reasons.push("proxy_missing".into());
    }
    if exists && !size_matches {
        reasons.push("size_mismatch".into());
    }
    if exists && !hash_matches {
        reasons.push("sha256_mismatch".into());
    }
    if exists && hash_matches && !decodes {
        reasons.push("decode_failed".into());
    }
    if decodes && !dimensions_match {
        reasons.push("dimensions_mismatch".into());
    }
    if !exists {
        reasons.push("no_decode_attempt".into());
    }
    ProxyValidation {
        exists,
        size_matches,
        hash_matches,
        decodes,
        dimensions_match,
        actual_sha256,
        actual_size,
        fingerprint,
        reasons,
    }
}

fn build_proxy_transaction(_store: &Store, meta: &mut AssetMeta) -> Result<(), String> {
    let source = Path::new(&meta.source_path);
    let actual_source_sha = hash::sha256_file(source)?;
    if actual_source_sha != meta.source_sha256 {
        return Err(format!(
            "源素材在恢复前已变化: expected={}, actual={actual_source_sha}",
            meta.source_sha256
        ));
    }
    let proxy = Path::new(&meta.proxy_path);
    if let Some(parent) = proxy.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = hash::sibling_temp(proxy, "proxy");
    if temp.exists() {
        std::fs::remove_file(&temp).map_err(|e| e.to_string())?;
    }
    let build = gst::build_proxy(source, &temp, &meta.proxy_spec);
    if let Err(error) = build {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let candidate = match inspect_candidate(&temp, &meta.proxy_spec) {
        Ok(value) => value,
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
    };
    std::fs::rename(&temp, proxy).map_err(|e| format!("原子发布代理失败: {e}"))?;
    meta.proxy_sha256 = Some(candidate.1);
    meta.proxy_size = Some(candidate.0);
    meta.decode_fingerprint = Some(candidate.2.raw_sha256);
    meta.state = ProxyState::Ready;
    meta.last_error = None;
    Ok(())
}

fn corrupt_proxy(store: &Store, meta: &mut AssetMeta) -> Result<(Option<String>, u64), String> {
    let proxy = Path::new(&meta.proxy_path);
    let original = std::fs::read(proxy).map_err(|e| format!("读取待破坏代理: {e}"))?;
    let expected_sha = meta.proxy_sha256.clone();
    let mut broken = b"BROKEN-PROXY\0".to_vec();
    let start = original.len().min(24);
    broken.extend_from_slice(&original[start..]);
    std::fs::write(proxy, &broken).map_err(|e| format!("写入失效代理: {e}"))?;
    let _actual_sha = hash::sha256_file(proxy)?;
    meta.state = ProxyState::Invalid;
    meta.last_action = "invalidated".into();
    meta.last_error = Some("proxy bytes corrupted for recovery test".into());
    store.save(meta)?;
    Ok((expected_sha, broken.len() as u64))
}

fn use_asset(store: &Store, id: &str) -> Result<UseOutcome, String> {
    let mut meta = store.load(id)?;
    let state_before = meta.state.as_str().to_string();
    let generation_before = meta.generation;
    let proxy_path = PathBuf::from(&meta.proxy_path);
    let mtime_before = hash::modified_ns(&proxy_path);

    if !extension_supported(Path::new(&meta.source_path)) {
        meta.state = ProxyState::Quarantined;
        meta.last_action = "rejected".into();
        meta.last_error = Some("unsupported_extension".into());
        store.save(&meta)?;
        return Ok(UseOutcome {
            asset_id: id.into(),
            decision: "rejected".into(),
            cache_hit: false,
            regenerated: false,
            generation_before,
            generation_after: meta.generation,
            state_before,
            state_after: meta.state.as_str().into(),
            transitions: vec!["missing".into(), "quarantined".into()],
            proxy_sha256: meta.proxy_sha256.clone(),
            decode_fingerprint: meta.decode_fingerprint.clone(),
            proxy_file_unchanged: mtime_before == hash::modified_ns(&proxy_path),
            reason: "unsupported_extension".into(),
        });
    }

    let validation = validate_existing(&meta);
    if meta.state == ProxyState::Ready && validation.is_valid() {
        let mtime_after = hash::modified_ns(&proxy_path);
        return Ok(UseOutcome {
            asset_id: id.into(),
            decision: "cache_hit".into(),
            cache_hit: true,
            regenerated: false,
            generation_before,
            generation_after: meta.generation,
            state_before,
            state_after: meta.state.as_str().into(),
            transitions: vec![meta.state.as_str().into()],
            proxy_sha256: meta.proxy_sha256.clone(),
            decode_fingerprint: meta.decode_fingerprint.clone(),
            proxy_file_unchanged: mtime_before == mtime_after,
            reason: "valid_proxy_and_matching_source".into(),
        });
    }

    let mut transitions = vec![state_before.clone()];
    if meta.state == ProxyState::Ready && !validation.is_valid() {
        meta.state = ProxyState::Invalid;
        meta.last_error = Some(format!("validation failed: {:?}", validation.reasons));
        store.save(&meta)?;
        transitions.push("invalid".into());
    }
    meta.state = ProxyState::Recovering;
    meta.last_action = "recovering".into();
    store.save(&meta)?;
    transitions.push("recovering".into());

    if let Err(error) = build_proxy_transaction(store, &mut meta) {
        meta.state = ProxyState::Invalid;
        meta.last_action = "recovery_failed".into();
        meta.last_error = Some(error.clone());
        let _ = store.save(&meta);
        return Err(error);
    }
    meta.generation += 1;
    meta.last_action = "recovered".into();
    store.save(&meta)?;
    transitions.push("ready".into());
    Ok(UseOutcome {
        asset_id: id.into(),
        decision: "recovered".into(),
        cache_hit: false,
        regenerated: true,
        generation_before,
        generation_after: meta.generation,
        state_before,
        state_after: meta.state.as_str().into(),
        transitions,
        proxy_sha256: meta.proxy_sha256.clone(),
        decode_fingerprint: meta.decode_fingerprint.clone(),
        proxy_file_unchanged: false,
        reason: "proxy_invalid_or_missing".into(),
    })
}

fn generate_source(
    store: &Store,
    id: &str,
    pattern: &str,
    config: &ScenarioConfig,
    j: &mut Journal,
    progress: &mut dyn FnMut(&str),
) -> Result<AssetMeta, String> {
    progress(&format!("生成源素材 {id}"));
    let source = store.source(id, "png");
    gst::generate_png_source(&source, pattern, config.source_width, config.source_height)?;
    let size = std::fs::metadata(&source).map_err(|e| e.to_string())?.len();
    let sha = hash::sha256_file(&source)?;
    let evidence = store.evidence(&format!("source-{id}.png"));
    hash::copy_evidence(&source, &evidence)?;
    let proxy = store.proxy(id);
    let meta = AssetMeta::new(
        id,
        source.clone(),
        proxy.clone(),
        sha.clone(),
        size,
        config.proxy_spec(),
    );
    store.save(&meta)?;
    j.append(
        &format!("generate-source-{id}"),
        "用 GStreamer 生成确定性 PNG 源素材并记录指纹",
        json!({
            "asset_id": id,
            "pattern": pattern,
            "source": store.relative(&source),
            "width": config.source_width,
            "height": config.source_height,
        }),
        "assets=0",
        "source=generated",
        json!({
            "source_sha256": sha,
            "source_size": size,
            "source_extension": "png",
            "evidence_sha256": hash::sha256_file(&evidence)?,
        }),
        vec![store.relative(&evidence)],
    )?;
    Ok(meta)
}

fn register_initial(
    store: &Store,
    id: &str,
    j: &mut Journal,
    progress: &mut dyn FnMut(&str),
) -> Result<(String, String), String> {
    progress(&format!("初次构建代理 {id}"));
    let mut meta = store.load(id)?;
    let before = meta.state.as_str().to_string();
    meta.state = ProxyState::Recovering;
    meta.last_action = "initial_build".into();
    store.save(&meta)?;
    build_proxy_transaction(store, &mut meta)?;
    meta.generation += 1;
    meta.last_action = "initial_build_complete".into();
    store.save(&meta)?;
    let proxy = PathBuf::from(&meta.proxy_path);
    let evidence = store.evidence(&format!("proxy-{id}-initial.png"));
    let evidence_sha = hash::copy_evidence(&proxy, &evidence)?;
    let fp = meta.decode_fingerprint.clone().unwrap_or_default();
    j.append(
        &format!("register-proxy-{id}"),
        "初次提交素材并生成降采样代理",
        json!({
            "asset_id": id,
            "source_sha256": meta.source_sha256,
            "proxy_spec": meta.proxy_spec,
        }),
        &format!("asset:{id}={before}"),
        &format!("asset:{id}=ready"),
        json!({
            "asset_id": id,
            "initial_build": true,
            "generation": meta.generation,
            "proxy_sha256": meta.proxy_sha256,
            "proxy_size": meta.proxy_size,
            "decode_fingerprint": fp,
            "evidence_sha256": evidence_sha,
            "transitions": ["missing", "recovering", "ready"],
        }),
        vec![store.relative(&evidence)],
    )?;
    Ok((meta.proxy_sha256.clone().unwrap_or_default(), fp))
}

fn invalidate_asset(
    store: &Store,
    id: &str,
    label: &str,
    j: &mut Journal,
    progress: &mut dyn FnMut(&str),
) -> Result<(String, String), String> {
    progress(&format!("使代理失效 {id}"));
    let mut meta = store.load(id)?;
    let before = meta.state.as_str().to_string();
    let expected = meta.proxy_sha256.clone().unwrap_or_default();
    let (_, broken_size) = corrupt_proxy(store, &mut meta)?;
    let proxy = PathBuf::from(&meta.proxy_path);
    let actual = hash::sha256_file(&proxy)?;
    let evidence = store.evidence(&format!("proxy-{id}-invalid-{label}.bin"));
    hash::copy_evidence(&proxy, &evidence)?;
    let meta_evidence = store.evidence(&format!("meta-{id}-invalid-{label}.json"));
    hash::copy_evidence(&store.meta(id), &meta_evidence)?;
    j.append(
        &format!("invalidate-{id}-{label}"),
        "破坏代理文件内容但保留元数据，制造失效状态",
        json!({
            "asset_id": id,
            "expected_proxy_sha256": expected,
            "corruption": "prefix BROKEN-PROXY and drop original first 24 bytes",
        }),
        &format!("asset:{id}={before}"),
        &format!("asset:{id}=invalid"),
        json!({
            "asset_id": id,
            "expected_sha256": expected,
            "actual_sha256": actual,
            "broken_size": broken_size,
            "hash_changed": expected != actual,
            "state_written": "invalid",
        }),
        vec![store.relative(&evidence), store.relative(&meta_evidence)],
    )?;
    Ok((expected, actual))
}

fn use_and_record(
    store: &Store,
    id: &str,
    step: &str,
    _purpose: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<(UseOutcome, Vec<String>), String> {
    progress(step);
    let outcome = use_asset(store, id)?;
    let after_meta = store.load(id)?;
    let mut evidence = Vec::new();
    if outcome.regenerated {
        let proxy = PathBuf::from(&after_meta.proxy_path);
        let file = format!("proxy-{id}-{step}.png");
        let path = store.evidence(&file);
        hash::copy_evidence(&proxy, &path)?;
        evidence.push(store.relative(&path));
    }
    let meta_copy = store.evidence(&format!("meta-{id}-{step}.json"));
    hash::copy_evidence(&store.meta(id), &meta_copy)?;
    evidence.push(store.relative(&meta_copy));
    Ok((outcome, evidence))
}

fn record_use(
    store: &Store,
    j: &mut Journal,
    id: &str,
    step: &str,
    purpose: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<UseOutcome, String> {
    let before = store.load(id)?;
    let state_before = before.state.as_str().to_string();
    let (outcome, evidence) = use_and_record(store, id, step, purpose, progress)?;
    let after = store.load(id)?;
    j.append(
        step,
        purpose,
        json!({
            "asset_id": id,
            "source_sha256": after.source_sha256,
            "proxy_spec": after.proxy_spec,
        }),
        &format!("asset:{id}={state_before}"),
        &format!("asset:{id}={}", outcome.state_after),
        serde_json::to_value(&outcome).map_err(|e| e.to_string())?,
        evidence,
    )?;
    Ok(outcome)
}

pub fn run_experiment(out_dir: &Path, progress: &mut dyn FnMut(&str)) -> Result<RunOutput, String> {
    run_experiment_with_config(out_dir, &ScenarioConfig::default(), progress)
}

pub fn run_experiment_with_config(
    out_dir: &Path,
    config: &ScenarioConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<RunOutput, String> {
    if let Err(error) = prepare_out_dir(out_dir) {
        return Err(error);
    }
    let result = run_inner(out_dir, config, progress);
    if let Err(error) = &result {
        let report = crate::report::render_failure_report(out_dir, config, error);
        let _ = hash::write_text_atomic(&out_dir.join("REPORT.md"), &report);
    }
    result
}

fn run_inner(
    out_dir: &Path,
    config: &ScenarioConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<RunOutput, String> {
    gst::init()?;
    let store = Store::new(out_dir);
    let mut journal = Journal::create(&out_dir.join("journal.jsonl"))?;

    let config_sha = hash::canonical_json_sha256(config)?;
    journal.append(
        "experiment-config",
        "固定实验参数，供回放时校验",
        serde_json::to_value(config).map_err(|e| e.to_string())?,
        "experiment=uninitialized",
        "experiment=configured",
        json!({"config_sha256": config_sha}),
        vec![],
    )?;

    progress("记录 GStreamer 环境");
    let env_path = gst::record_gst_env(&store.evidence("gst-env.txt"))?;
    let env_text = std::fs::read_to_string(&env_path).map_err(|e| e.to_string())?;
    let gst_version = env_text
        .lines()
        .find_map(|line| line.strip_prefix("gstreamer_version="))
        .unwrap_or("unknown")
        .to_string();
    journal.append(
        "gst-env",
        "记录 GStreamer 版本与关键元素，作为环境证据",
        json!({}),
        "experiment=configured",
        "environment=captured",
        json!({
            "gstreamer_version": gst_version,
            "all_required_elements_present": !env_text.contains("=MISSING"),
        }),
        vec![store.relative(&env_path)],
    )?;

    let _meta_a = generate_source(
        &store,
        ASSET_A,
        &config.pattern_a,
        config,
        &mut journal,
        progress,
    )?;
    let _meta_b = generate_source(
        &store,
        ASSET_B,
        &config.pattern_b,
        config,
        &mut journal,
        progress,
    )?;
    let (initial_a_sha, initial_a_fp) = register_initial(&store, ASSET_A, &mut journal, progress)?;
    let (initial_b_sha, initial_b_fp) = register_initial(&store, ASSET_B, &mut journal, progress)?;

    let (_expected_a, invalid_a_sha) =
        invalidate_asset(&store, ASSET_A, "initial", &mut journal, progress)?;
    let (_expected_b, invalid_b_sha) =
        invalidate_asset(&store, ASSET_B, "initial", &mut journal, progress)?;

    let recover_a = record_use(
        &store,
        &mut journal,
        ASSET_A,
        "recover-clip-a",
        "失效后首次恢复：重建并用 GStreamer 解码校验代理",
        progress,
    )?;
    let recover_b = record_use(
        &store,
        &mut journal,
        ASSET_B,
        "recover-clip-b",
        "失效后首次恢复：重建 B，建立原始恢复顺序 A → B",
        progress,
    )?;
    let post_replay = record_use(
        &store,
        &mut journal,
        ASSET_A,
        "post-recovery-replay-clip-a",
        "恢复完成后按同一输入重放一次，预期命中缓存且不重新生成",
        progress,
    )?;

    progress("提交未知扩展对照");
    let control_source = store.source(CONTROL, &config.unknown_extension);
    std::fs::copy(store.source(ASSET_A, "png"), &control_source)
        .map_err(|e| format!("创建未知扩展对照源: {e}"))?;
    let control_sha = hash::sha256_file(&control_source)?;
    let control_size = std::fs::metadata(&control_source)
        .map_err(|e| e.to_string())?
        .len();
    let control_meta = AssetMeta::new(
        CONTROL,
        control_source.clone(),
        store.proxy(CONTROL),
        control_sha.clone(),
        control_size,
        config.proxy_spec(),
    );
    store.save(&control_meta)?;
    let control_outcome = use_asset(&store, CONTROL)?;
    let control_after = store.load(CONTROL)?;
    let control_meta_evidence = store.evidence("meta-control-unknown.json");
    hash::copy_evidence(&store.meta(CONTROL), &control_meta_evidence)?;
    let control_proxy_created = Path::new(&control_after.proxy_path).exists();
    journal.append(
        "unknown-extension-control",
        "相同字节内容换未知扩展提交，预期在 GStreamer 之前拒绝",
        json!({
            "asset_id": CONTROL,
            "extension": config.unknown_extension,
            "source_sha256": control_sha,
            "content_same_as": ASSET_A,
        }),
        "asset:control-unknown=missing",
        "asset:control-unknown=quarantined",
        json!({
            "asset_id": CONTROL,
            "accepted": false,
            "decision": control_outcome.decision,
            "error": control_outcome.reason,
            "extension": extension_of(&control_source),
            "content_sha256_equals_clip_a": control_sha == store.load(ASSET_A)?.source_sha256,
            "source_sha256_equals_clip_a": control_sha == store.load(ASSET_A)?.source_sha256,
            "gst_decode_invoked": false,
            "proxy_created": control_proxy_created,
            "state_written": control_after.state.as_str(),
        }),
        vec![store.relative(&control_meta_evidence)],
    )?;

    invalidate_asset(&store, ASSET_B, "for-order", &mut journal, progress)?;
    invalidate_asset(&store, ASSET_A, "for-order", &mut journal, progress)?;
    let reversed_b = record_use(
        &store,
        &mut journal,
        ASSET_B,
        "recover-clip-b-reversed",
        "顺序改变验证：先恢复 B",
        progress,
    )?;
    let reversed_a = record_use(
        &store,
        &mut journal,
        ASSET_A,
        "recover-clip-a-reversed",
        "顺序改变验证：后恢复 A（与首次 A → B 相反）",
        progress,
    )?;

    let order_matches = recovered_matches(&recover_a, &reversed_a, &initial_a_sha, &initial_a_fp)
        && recovered_matches(&recover_b, &reversed_b, &initial_b_sha, &initial_b_fp);
    journal.append(
        "verify-order",
        "对比 A→B 与 B→A 两种恢复顺序的代理哈希和解码指纹",
        json!({"first_order": [ASSET_A, ASSET_B], "reversed_order": [ASSET_B, ASSET_A]}),
        "both_assets=ready_after_reversed_recovery",
        if order_matches {
            "order_check=passed"
        } else {
            "order_check=failed"
        },
        json!({
            "matches_reference": order_matches,
            "clip_a": {
                "first_recovery_sha256": recover_a.proxy_sha256,
                "reversed_recovery_sha256": reversed_a.proxy_sha256,
                "initial_sha256": initial_a_sha,
                "first_fingerprint": recover_a.decode_fingerprint,
                "reversed_fingerprint": reversed_a.decode_fingerprint,
            },
            "clip_b": {
                "first_recovery_sha256": recover_b.proxy_sha256,
                "reversed_recovery_sha256": reversed_b.proxy_sha256,
                "initial_sha256": initial_b_sha,
                "first_fingerprint": recover_b.decode_fingerprint,
                "reversed_fingerprint": reversed_b.decode_fingerprint,
            }
        }),
        vec![],
    )?;

    let repeat_1 = record_use(
        &store,
        &mut journal,
        ASSET_A,
        "repeat-submit-clip-a-1",
        "重复提交验证：第 1 次提交已恢复素材",
        progress,
    )?;
    let repeat_2 = record_use(
        &store,
        &mut journal,
        ASSET_A,
        "repeat-submit-clip-a-2",
        "重复提交验证：第 2 次提交同一素材，预期仍为缓存命中",
        progress,
    )?;
    let repeat_ok = repeat_1.cache_hit
        && repeat_2.cache_hit
        && !repeat_1.regenerated
        && !repeat_2.regenerated
        && repeat_1.generation_before == repeat_1.generation_after
        && repeat_2.generation_before == repeat_2.generation_after
        && repeat_1.generation_after == repeat_2.generation_after
        && repeat_1.proxy_sha256 == repeat_2.proxy_sha256
        && repeat_1.proxy_sha256 == reversed_a.proxy_sha256
        && repeat_1.proxy_file_unchanged
        && repeat_2.proxy_file_unchanged;
    journal.append(
        "verify-repeat",
        "对比两次重复提交的缓存命中、generation、文件哈希和文件修改状态",
        json!({"asset_id": ASSET_A, "submissions": 2}),
        "asset:clip-a=ready",
        if repeat_ok {
            "repeat_check=passed"
        } else {
            "repeat_check=failed"
        },
        json!({
            "idempotent": repeat_ok,
            "first": {
                "cache_hit": repeat_1.cache_hit,
                "generation": repeat_1.generation_after,
                "proxy_sha256": repeat_1.proxy_sha256,
                "file_unchanged": repeat_1.proxy_file_unchanged,
            },
            "second": {
                "cache_hit": repeat_2.cache_hit,
                "generation": repeat_2.generation_after,
                "proxy_sha256": repeat_2.proxy_sha256,
                "file_unchanged": repeat_2.proxy_file_unchanged,
            }
        }),
        vec![],
    )?;

    progress("计算自动结论");
    let final_a = store.load(ASSET_A)?;
    let final_b = store.load(ASSET_B)?;
    let valid_a = validate_existing(&final_a);
    let valid_b = validate_existing(&final_b);
    let mut checks = Vec::new();
    checks.push(Check {
        name: "recovery-restores-valid-proxy".into(),
        passed: recover_a.state_after == "ready"
            && recover_b.state_after == "ready"
            && recover_a.proxy_sha256.as_deref() == Some(initial_a_sha.as_str())
            && recover_b.proxy_sha256.as_deref() == Some(initial_b_sha.as_str())
            && valid_a.is_valid()
            && valid_b.is_valid(),
        detail: format!(
            "A sha {} -> {}; B sha {} -> {}; final_valid_a={} final_valid_b={}",
            initial_a_sha,
            recover_a.proxy_sha256.clone().unwrap_or_default(),
            initial_b_sha,
            recover_b.proxy_sha256.clone().unwrap_or_default(),
            valid_a.is_valid(),
            valid_b.is_valid()
        ),
    });
    checks.push(Check {
        name: "post-recovery-replay-no-regeneration".into(),
        passed: post_replay.cache_hit
            && !post_replay.regenerated
            && post_replay.generation_before == post_replay.generation_after
            && post_replay.proxy_file_unchanged
            && post_replay.proxy_sha256 == recover_a.proxy_sha256,
        detail: format!(
            "decision={} generation={}->{} file_unchanged={} sha={}",
            post_replay.decision,
            post_replay.generation_before,
            post_replay.generation_after,
            post_replay.proxy_file_unchanged,
            post_replay.proxy_sha256.clone().unwrap_or_default()
        ),
    });
    checks.push(Check {
        name: "unknown-extension-control-quarantined".into(),
        passed: control_after.state == ProxyState::Quarantined
            && control_outcome.reason == "unsupported_extension"
            && !control_proxy_created
            && control_sha == store.load(ASSET_A)?.source_sha256,
        detail: format!(
            "extension={} state={} proxy_created={} same_content_as_A={}",
            config.unknown_extension,
            control_after.state.as_str(),
            control_proxy_created,
            control_sha == store.load(ASSET_A)?.source_sha256
        ),
    });
    checks.push(Check {
        name: "recovery-order-independent".into(),
        passed: order_matches,
        detail: format!(
            "A->B vs B->A: A sha {} vs {}, B sha {} vs {}",
            recover_a.proxy_sha256.clone().unwrap_or_default(),
            reversed_a.proxy_sha256.clone().unwrap_or_default(),
            recover_b.proxy_sha256.clone().unwrap_or_default(),
            reversed_b.proxy_sha256.clone().unwrap_or_default()
        ),
    });
    checks.push(Check {
        name: "duplicate-submission-idempotent".into(),
        passed: repeat_ok,
        detail: format!(
            "submissions cache_hit={}/{} file_unchanged={}/{} generation={}/{} sha={}",
            repeat_1.cache_hit,
            repeat_2.cache_hit,
            repeat_1.proxy_file_unchanged,
            repeat_2.proxy_file_unchanged,
            repeat_1.generation_after,
            repeat_2.generation_after,
            repeat_1.proxy_sha256.clone().unwrap_or_default()
        ),
    });

    let journal_records = read_journal(journal.path())?;
    let required_steps = [
        "experiment-config",
        "generate-source-clip-a",
        "register-proxy-clip-a",
        "invalidate-clip-a-initial",
        "recover-clip-a",
        "post-recovery-replay-clip-a",
        "unknown-extension-control",
        "verify-order",
        "verify-repeat",
    ];
    let journal_complete = required_steps
        .iter()
        .all(|step| journal_records.iter().any(|r| r.step == *step))
        && journal_records.iter().all(|r| {
            !r.purpose.is_empty()
                && !r.input.is_null()
                && !r.result.is_null()
                && !r.state_before.is_empty()
                && !r.state_after.is_empty()
        });
    checks.push(Check {
        name: "journal-and-key-evidence-retained".into(),
        passed: journal_complete
            && invalid_a_sha != initial_a_sha
            && invalid_b_sha != initial_b_sha
            && store.evidence("proxy-clip-a-initial.png").is_file()
            && store.evidence("proxy-clip-a-invalid-initial.bin").is_file()
            && store.evidence("proxy-clip-a-recover-clip-a.png").is_file()
            && store.evidence("meta-control-unknown.json").is_file()
            && store.evidence("gst-env.txt").is_file(),
        detail: format!(
            "records={} required_steps_present={} invalid_hashes_changed={}/{}",
            journal_records.len(),
            journal_records
                .iter()
                .filter(|r| required_steps.contains(&r.step.as_str()))
                .count(),
            invalid_a_sha != initial_a_sha,
            invalid_b_sha != initial_b_sha
        ),
    });

    let all_passed = checks.iter().all(|check| check.passed);
    journal.append(
        "analyze",
        "形成自动结论；自动结论仅覆盖可计算的状态与哈希约束",
        json!({"check_count": checks.len()}),
        "experiment=all_steps_executed",
        if all_passed {
            "experiment=automated_checks_passed"
        } else {
            "experiment=automated_checks_failed"
        },
        json!({"checks": checks, "all_passed": all_passed}),
        vec![],
    )?;

    progress("生成报告");
    let report_path = out_dir.join("REPORT.md");
    let report = crate::report::render_report(out_dir, journal.path(), config, &checks, all_passed);
    hash::write_text_atomic(&report_path, &report)?;
    journal.append(
        "report",
        "生成自动结论与人工确认分栏的报告",
        json!({"format": "markdown"}),
        "experiment=analyzed",
        "report=written",
        json!({
            "all_passed": all_passed,
            "automatic_section": true,
            "manual_section": true,
            "failure_actions_section": true,
        }),
        vec![store.relative(&report_path)],
    )?;

    let manifest_path = store.evidence("manifest.json");
    let sealed_count = count_files(out_dir)?;
    journal.append(
        "seal-evidence",
        "对保留的关键证据与日志建立 SHA-256 清单",
        json!({"exclude": ["evidence/manifest.json", "replay/"]}),
        "report=written",
        "evidence=sealed",
        json!({
            "manifest": store.relative(&manifest_path),
            "sealed_file_count": sealed_count,
        }),
        vec![store.relative(&manifest_path)],
    )?;
    hash::write_json_atomic(
        &manifest_path,
        &crate::report::evidence_manifest(out_dir, &manifest_path)?,
    )?;

    Ok(RunOutput {
        journal_path: journal.path().to_path_buf(),
        report_path,
        manifest_path,
        checks,
        all_passed,
    })
}

fn recovered_matches(
    first: &UseOutcome,
    reversed: &UseOutcome,
    initial_sha: &str,
    initial_fp: &str,
) -> bool {
    first.state_after == "ready"
        && reversed.state_after == "ready"
        && first.proxy_sha256.as_deref() == Some(initial_sha)
        && reversed.proxy_sha256.as_deref() == Some(initial_sha)
        && first.decode_fingerprint.as_deref() == Some(initial_fp)
        && reversed.decode_fingerprint.as_deref() == Some(initial_fp)
}

fn count_files(root: &Path) -> Result<usize, String> {
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).map_err(|e| format!("遍历证据目录 {}: {e}", dir.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                count += 1;
            }
        }
    }
    Ok(count)
}

pub fn run_summary(output: &RunOutput) -> String {
    let mut text = format!(
        "journal: {}\nreport: {}\nmanifest: {}\nchecks:\n",
        output.journal_path.display(),
        output.report_path.display(),
        output.manifest_path.display()
    );
    for check in &output.checks {
        text.push_str(&format!(
            "  [{}] {} - {}\n",
            if check.passed { "PASS" } else { "FAIL" },
            check.name,
            check.detail
        ));
    }
    text
}
