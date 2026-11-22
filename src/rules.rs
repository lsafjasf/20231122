//! 冲突检测与修复建议规则。
//!
//! 每条规则检查 `FrameMeta` 的当前值，产出 `Issue`：
//! 描述冲突关系（哪些字段互相矛盾 / 哪些字段缺失），
//! 并给出逐字段的修复建议（含修复依据）。

use crate::meta::*;

/// 单字段修复建议。
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub field: FieldKey,
    pub value: FieldValue,
    /// 修复依据（规范条款/推导过程），写入报告。
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// 字段之间互相矛盾。
    Conflict,
    /// 声明的格式要求该字段存在，但帧里缺失。
    Missing,
}

impl IssueKind {
    pub fn label(self) -> &'static str {
        match self {
            IssueKind::Conflict => "冲突",
            IssueKind::Missing => "缺字段",
        }
    }
}

/// 一个帧上发现的问题。
#[derive(Debug, Clone)]
pub struct Issue {
    pub frame_no: u64,
    pub kind: IssueKind,
    /// 参与冲突/缺失的字段。
    pub fields: Vec<FieldKey>,
    /// 冲突关系描述。
    pub description: String,
    pub suggestions: Vec<Suggestion>,
}

impl Issue {
    fn conflict(frame_no: u64, fields: Vec<FieldKey>, description: String) -> Self {
        Issue {
            frame_no,
            kind: IssueKind::Conflict,
            fields,
            description,
            suggestions: Vec::new(),
        }
    }

    fn missing(frame_no: u64, field: FieldKey, description: String) -> Self {
        Issue {
            frame_no,
            kind: IssueKind::Missing,
            fields: vec![field],
            description,
            suggestions: Vec::new(),
        }
    }

    fn suggest(&mut self, field: FieldKey, value: FieldValue, rationale: &str) {
        self.suggestions.push(Suggestion {
            field,
            value,
            rationale: rationale.to_string(),
        });
    }
}

/// 由 EV 推导场景参考亮度（cd/m²）：ISO 100、校准常数 K=12.5。
pub fn ev_to_luminance(ev: f64) -> f64 {
    0.125 * 2f64.powf(ev)
}

/// 把亮度向上取整到常用母版峰值档位。
pub fn standard_peak_for(luminance: f64) -> f64 {
    [1000.0, 4000.0, 10000.0]
        .into_iter()
        .find(|&p| luminance <= p)
        .unwrap_or(10000.0)
}

/// 对一帧运行全部规则，返回问题列表（基于当前值，修复后可复跑收敛）。
pub fn analyze(frame: &FrameMeta) -> Vec<Issue> {
    let mut issues = Vec::new();
    check_format_vs_transfer(frame, &mut issues);
    check_transfer_vs_colorimetry(frame, &mut issues);
    check_matrix_vs_range(frame, &mut issues);
    check_light_levels(frame, &mut issues);
    check_exposure_vs_mastering(frame, &mut issues);
    check_missing_fields(frame, &mut issues);
    issues
}

/// 规则 1/2/3：HDR 格式标签与传递函数必须一致。
fn check_format_vs_transfer(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let format = match frame.get(FieldKey::HdrFormat) {
        Some(FieldValue::Format(f)) => f,
        _ => return,
    };
    let transfer = match frame.get(FieldKey::Transfer) {
        Some(FieldValue::Transfer(t)) => Some(t),
        _ => None,
    };
    let no = frame.frame_no;
    match (format, transfer) {
        (HdrFormat::Hdr10, Some(t)) if t != Transfer::Pq => {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::HdrFormat, FieldKey::Transfer],
                format!("标签声明 HDR10，但传递函数是 {t}，二者矛盾"),
            );
            i.suggest(
                FieldKey::Transfer,
                FieldValue::Transfer(Transfer::Pq),
                "HDR10（ST 2086/2094 体系）强制使用 SMPTE ST 2084 PQ 传递函数",
            );
            issues.push(i);
        }
        (HdrFormat::Hlg, Some(t)) if t != Transfer::Hlg => {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::HdrFormat, FieldKey::Transfer],
                format!("标签声明 HLG，但传递函数是 {t}，二者矛盾"),
            );
            i.suggest(
                FieldKey::Transfer,
                FieldValue::Transfer(Transfer::Hlg),
                "HLG 内容必须使用 ARIB STD-B67 (HLG) 传递函数",
            );
            issues.push(i);
        }
        (HdrFormat::Sdr, Some(t @ (Transfer::Pq | Transfer::Hlg))) => {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::HdrFormat, FieldKey::Transfer],
                format!("标签声明 SDR，但传递函数 {t} 属于 HDR，二者矛盾"),
            );
            let (fmt, why) = if t == Transfer::Pq {
                (HdrFormat::Hdr10, "PQ 传递函数对应 HDR10 格式标签")
            } else {
                (HdrFormat::Hlg, "HLG 传递函数对应 HLG 格式标签")
            };
            i.suggest(FieldKey::HdrFormat, FieldValue::Format(fmt), why);
            issues.push(i);
        }
        _ => {}
    }
}

/// 规则 4：HDR 传递函数必须搭配 BT.2020 基色/矩阵。
fn check_transfer_vs_colorimetry(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let hdr_transfer = matches!(
        frame.get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Pq | Transfer::Hlg))
    );
    if !hdr_transfer {
        return;
    }
    let no = frame.frame_no;
    if let Some(FieldValue::Primaries(p)) = frame.get(FieldKey::Primaries) {
        if p != Primaries::Bt2020 {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::Transfer, FieldKey::Primaries],
                format!("HDR 传递函数搭配 {p} 基色，色域容器不足"),
            );
            i.suggest(
                FieldKey::Primaries,
                FieldValue::Primaries(Primaries::Bt2020),
                "HDR10/HLG 均规定使用 BT.2020 色度基色（BT.2100 表 2）",
            );
            issues.push(i);
        }
    }
    if let Some(FieldValue::Matrix(m)) = frame.get(FieldKey::Matrix) {
        if m == Matrix::Bt709 {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::Transfer, FieldKey::Matrix],
                "HDR 传递函数搭配 bt709 矩阵，与 BT.2100 规定矛盾".to_string(),
            );
            i.suggest(
                FieldKey::Matrix,
                FieldValue::Matrix(Matrix::Bt2020Ncl),
                "BT.2100 规定 HDR 使用 BT.2020 非恒定亮度矩阵",
            );
            issues.push(i);
        }
    }
}

/// 规则 5：BT.2020 恒定亮度矩阵只允许 limited range。
fn check_matrix_vs_range(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let cl = matches!(
        frame.get(FieldKey::Matrix),
        Some(FieldValue::Matrix(Matrix::Bt2020Cl))
    );
    let full = matches!(
        frame.get(FieldKey::Range),
        Some(FieldValue::Range(Range::Full))
    );
    if cl && full {
        let mut i = Issue::conflict(
            frame.frame_no,
            vec![FieldKey::Matrix, FieldKey::Range],
            "BT.2020 恒定亮度矩阵搭配 full range，超出规范定义".to_string(),
        );
        i.suggest(
            FieldKey::Range,
            FieldValue::Range(Range::Limited),
            "BT.2020 CL 矩阵仅在 limited range 下定义（BT.2020 表 4）",
        );
        issues.push(i);
    }
}

/// 规则 6/7：静态亮度元数据内部的数值矛盾。
fn check_light_levels(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let no = frame.frame_no;
    let cll = num(frame, FieldKey::MaxCll);
    let fall = num(frame, FieldKey::MaxFall);
    let peak = num(frame, FieldKey::MasteringMaxLum);
    if let (Some(fall), Some(cll)) = (fall, cll) {
        if fall > cll {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::MaxFall, FieldKey::MaxCll],
                format!("MaxFALL {fall} 大于 MaxCLL {cll}，帧平均不可能超过帧峰值"),
            );
            i.suggest(
                FieldKey::MaxFall,
                FieldValue::Number(cll),
                "MaxFALL 按定义不超过 MaxCLL（CTA-861.3），取 MaxCLL 为上限",
            );
            issues.push(i);
        }
    }
    if let (Some(cll), Some(peak)) = (cll, peak) {
        if cll > peak {
            let mut i = Issue::conflict(
                no,
                vec![FieldKey::MaxCll, FieldKey::MasteringMaxLum],
                format!("MaxCLL {cll} 超过母版峰值亮度 {peak}，内容超出母版能力"),
            );
            i.suggest(
                FieldKey::MaxCll,
                FieldValue::Number(peak),
                "MaxCLL 不应超过母版显示器峰值（ST 2086），钳制到母版峰值",
            );
            issues.push(i);
        }
    }
}

/// 规则 8：曝光值隐含的场景亮度与母版峰值矛盾。
fn check_exposure_vs_mastering(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let (Some(ev), Some(peak)) = (
        num(frame, FieldKey::ExposureValue),
        num(frame, FieldKey::MasteringMaxLum),
    ) else {
        return;
    };
    let scene = ev_to_luminance(ev);
    if scene > peak {
        let target = standard_peak_for(scene);
        let mut i = Issue::conflict(
            frame.frame_no,
            vec![FieldKey::ExposureValue, FieldKey::MasteringMaxLum],
            format!("EV {ev} 隐含场景亮度约 {scene:.0} cd/m²，超过母版峰值 {peak} cd/m²"),
        );
        i.suggest(
            FieldKey::MasteringMaxLum,
            FieldValue::Number(target),
            "按 EV= log2(L·S/K)（ISO 100, K=12.5）反推场景亮度，母版峰值上提到标准档位",
        );
        issues.push(i);
    }
}

fn num(frame: &FrameMeta, key: FieldKey) -> Option<f64> {
    match frame.get(key) {
        Some(FieldValue::Number(v)) => Some(v),
        _ => None,
    }
}

/// 规则 9：声明格式要求的字段缺失时，按规范默认值补齐。
fn check_missing_fields(frame: &FrameMeta, issues: &mut Vec<Issue>) {
    let no = frame.frame_no;
    let format = match frame.get(FieldKey::HdrFormat) {
        Some(FieldValue::Format(f)) => Some(f),
        _ => None,
    };
    // 标签缺失但传递函数已表明 HDR：补标签。
    if format.is_none() {
        match frame.get(FieldKey::Transfer) {
            Some(FieldValue::Transfer(Transfer::Pq)) => {
                let mut i = Issue::missing(
                    no,
                    FieldKey::HdrFormat,
                    "传递函数为 PQ 但缺少 HDR 格式标签".to_string(),
                );
                i.suggest(
                    FieldKey::HdrFormat,
                    FieldValue::Format(HdrFormat::Hdr10),
                    "PQ + 静态元数据体系对应 HDR10 标签",
                );
                issues.push(i);
            }
            Some(FieldValue::Transfer(Transfer::Hlg)) => {
                let mut i = Issue::missing(
                    no,
                    FieldKey::HdrFormat,
                    "传递函数为 HLG 但缺少 HDR 格式标签".to_string(),
                );
                i.suggest(
                    FieldKey::HdrFormat,
                    FieldValue::Format(HdrFormat::Hlg),
                    "HLG 传递函数对应 HLG 标签",
                );
                issues.push(i);
            }
            _ => {}
        }
        return;
    }
    let required: &[(FieldKey, FieldValue, &str)] = match format.unwrap() {
        HdrFormat::Hdr10 => &[
            (FieldKey::Transfer, FieldValue::Transfer(Transfer::Pq), "HDR10 默认 PQ"),
            (FieldKey::Primaries, FieldValue::Primaries(Primaries::Bt2020), "HDR10 默认 BT.2020"),
            (FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt2020Ncl), "HDR10 默认 BT.2020 NCL"),
            (FieldKey::Range, FieldValue::Range(Range::Limited), "消费端 HDR10 通常为 limited"),
            (FieldKey::MasteringMaxLum, FieldValue::Number(1000.0), "ST 2086 常用母版峰值 1000 cd/m²"),
            (FieldKey::MasteringMinLum, FieldValue::Number(0.0001), "ST 2086 常用母版黑位 0.0001 cd/m²"),
            (FieldKey::MaxCll, FieldValue::Number(1000.0), "CTA-861.3 要求 MaxCLL，缺省取母版峰值"),
            (FieldKey::MaxFall, FieldValue::Number(1000.0), "CTA-861.3 要求 MaxFALL，缺省取 MaxCLL"),
        ],
        HdrFormat::Hlg => &[
            (FieldKey::Transfer, FieldValue::Transfer(Transfer::Hlg), "HLG 默认 HLG 传递函数"),
            (FieldKey::Primaries, FieldValue::Primaries(Primaries::Bt2020), "HLG 默认 BT.2020"),
            (FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt2020Ncl), "HLG 默认 BT.2020 NCL"),
        ],
        HdrFormat::Sdr => &[],
    };
    for &(key, default, why) in required {
        if frame.get(key).is_none() {
            let mut i = Issue::missing(
                no,
                key,
                format!("声明为 {} 但缺少字段「{}」", format.unwrap(), key.label()),
            );
            i.suggest(key, default, why);
            issues.push(i);
        }
    }
}
