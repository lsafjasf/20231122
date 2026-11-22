use gtk4::prelude::*;
use rotprov_core::model::{Conclusion, Evidence, EvidenceKind, ProvenanceEntry, Snapshot};

/// 渲染单个对象的完整来源链：结论 → 快照 → 处理记录 → 证据。
pub fn render_entry(container: &gtk4::Box, entry: &ProvenanceEntry) {
    let title = gtk4::Label::new(Some(&format!(
        "{}（{}）",
        entry.manifest.name, entry.manifest.id
    )));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("title-3");
    container.append(&title);

    let conc = gtk4::Label::new(Some(&format!("结论：{}", entry.conclusion.label())));
    conc.set_halign(gtk4::Align::Start);
    conc.add_css_class(match &entry.conclusion {
        Conclusion::Fixed { .. } => "conc-ok",
        Conclusion::NoFixNeeded => "conc-skip",
        Conclusion::Undecidable { .. } => "conc-undec",
        Conclusion::Failed { .. } => "conc-fail",
    });
    container.append(&conc);

    if let Conclusion::Undecidable { reason } = &entry.conclusion {
        let why = gtk4::Label::new(Some(&format!("边界说明：{reason}")));
        why.set_halign(gtk4::Align::Start);
        why.add_css_class("dim");
        container.append(&why);
    }
    if let Conclusion::Failed { error } = &entry.conclusion {
        let why = gtk4::Label::new(Some(&format!("异常：{error}")));
        why.set_halign(gtk4::Align::Start);
        why.add_css_class("dim");
        why.set_wrap(true);
        container.append(&why);
    }

    render_snapshots(container, &entry.snapshots);
    render_record(container, entry);
    render_evidence(container, &entry.evidence);
}

fn section(container: &gtk4::Box, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.set_halign(gtk4::Align::Start);
    label.add_css_class("section");
    container.append(&label);
}

fn mono_line(container: &gtk4::Box, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.set_halign(gtk4::Align::Start);
    label.set_wrap(true);
    label.set_selectable(true);
    label.add_css_class("mono");
    container.append(&label);
}

fn render_snapshots(container: &gtk4::Box, snapshots: &[Snapshot]) {
    section(container, "版本快照");
    if snapshots.is_empty() {
        mono_line(container, "（无快照：对象在 ingest 阶段前失败）");
        return;
    }
    for snap in snapshots {
        let json = serde_json::to_string_pretty(&snap.state).unwrap_or_default();
        mono_line(
            container,
            &format!("v{} · {} 阶段\n{}", snap.version, snap.stage, json),
        );
    }
}

fn render_record(container: &gtk4::Box, entry: &ProvenanceEntry) {
    let record = &entry.record;
    section(container, "处理记录");
    if record.truncated {
        let badge = gtk4::Label::new(Some(&format!(
            "⚠ 记录已截断：保留 {}/{} 步 · 原始哈希 {}",
            record.steps.len(),
            record.original_step_count,
            record.original_hash.as_deref().unwrap_or("?")
        )));
        badge.set_halign(gtk4::Align::Start);
        badge.add_css_class("trunc-badge");
        container.append(&badge);
    }
    for step in &record.steps {
        let refs = if step.evidence_ids.is_empty() {
            String::new()
        } else {
            format!("  [证据: {}]", step.evidence_ids.join(", "))
        };
        mono_line(
            container,
            &format!("#{:03} {} — {}{}", step.seq, step.name, step.detail, refs),
        );
    }
}

fn render_evidence(container: &gtk4::Box, evidence: &[Evidence]) {
    section(container, "关键证据");
    if evidence.is_empty() {
        mono_line(container, "（无证据）");
        return;
    }
    for item in evidence {
        let kind = match item.kind {
            EvidenceKind::RawInput => "原始输入",
            EvidenceKind::GstProbe => "GStreamer 探测",
            EvidenceKind::GstTagList => "GStreamer 标签",
            EvidenceKind::ConflictingValues => "冲突原值",
            EvidenceKind::ErrorContext => "异常现场",
        };
        mono_line(
            container,
            &format!("{} [{}] {}\n{}", item.id, kind, item.summary, item.content),
        );
    }
}
