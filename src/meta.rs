//! HDR 帧元数据的核心数据模型。
//!
//! 每个字段都是 `Option`，`None` 表示该帧缺失此字段。
//! 模型同时保留"原始值"与"当前值"，修复只改当前值，
//! 以便导出修复前后的对照清单。

use std::fmt;

/// 码流/容器上声明的 HDR 格式标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HdrFormat {
    Sdr,
    Hdr10,
    Hlg,
}

impl fmt::Display for HdrFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            HdrFormat::Sdr => "SDR",
            HdrFormat::Hdr10 => "HDR10",
            HdrFormat::Hlg => "HLG",
        };
        f.write_str(s)
    }
}

/// 传递函数（transfer characteristics）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transfer {
    Bt709,
    Srgb,
    /// SMPTE ST 2084，HDR10 使用的 PQ。
    Pq,
    /// ARIB STD-B67，HLG。
    Hlg,
}

impl fmt::Display for Transfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Transfer::Bt709 => "bt709",
            Transfer::Srgb => "srgb",
            Transfer::Pq => "smpte2084 (PQ)",
            Transfer::Hlg => "hlg",
        };
        f.write_str(s)
    }
}

/// 色度基色（primaries）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primaries {
    Bt709,
    Bt2020,
}

impl fmt::Display for Primaries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Primaries::Bt709 => "bt709",
            Primaries::Bt2020 => "bt2020",
        };
        f.write_str(s)
    }
}

/// 色彩矩阵系数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Matrix {
    Bt709,
    /// BT.2020 非恒定亮度。
    Bt2020Ncl,
    /// BT.2020 恒定亮度。
    Bt2020Cl,
}

impl fmt::Display for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Matrix::Bt709 => "bt709",
            Matrix::Bt2020Ncl => "bt2020-ncl",
            Matrix::Bt2020Cl => "bt2020-cl",
        };
        f.write_str(s)
    }
}

/// 量化范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Range {
    Limited,
    Full,
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Range::Limited => "limited",
            Range::Full => "full",
        };
        f.write_str(s)
    }
}

/// 可被检查/修复的字段标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldKey {
    HdrFormat,
    Transfer,
    Primaries,
    Matrix,
    Range,
    MasteringMaxLum,
    MasteringMinLum,
    MaxCll,
    MaxFall,
    ExposureValue,
}

impl FieldKey {
    pub const ALL: [FieldKey; 10] = [
        FieldKey::HdrFormat,
        FieldKey::Transfer,
        FieldKey::Primaries,
        FieldKey::Matrix,
        FieldKey::Range,
        FieldKey::MasteringMaxLum,
        FieldKey::MasteringMinLum,
        FieldKey::MaxCll,
        FieldKey::MaxFall,
        FieldKey::ExposureValue,
    ];

    /// 界面与报告里使用的中文名。
    pub fn label(self) -> &'static str {
        match self {
            FieldKey::HdrFormat => "HDR 格式标签",
            FieldKey::Transfer => "传递函数",
            FieldKey::Primaries => "色度基色",
            FieldKey::Matrix => "色彩矩阵",
            FieldKey::Range => "量化范围",
            FieldKey::MasteringMaxLum => "母版峰值亮度 (cd/m²)",
            FieldKey::MasteringMinLum => "母版最低亮度 (cd/m²)",
            FieldKey::MaxCll => "MaxCLL (cd/m²)",
            FieldKey::MaxFall => "MaxFALL (cd/m²)",
            FieldKey::ExposureValue => "曝光值 EV",
        }
    }
}

/// 字段值，统一成可比较、可显示的枚举。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldValue {
    Format(HdrFormat),
    Transfer(Transfer),
    Primaries(Primaries),
    Matrix(Matrix),
    Range(Range),
    /// 亮度等浮点量。
    Number(f64),
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldValue::Format(v) => write!(f, "{v}"),
            FieldValue::Transfer(v) => write!(f, "{v}"),
            FieldValue::Primaries(v) => write!(f, "{v}"),
            FieldValue::Matrix(v) => write!(f, "{v}"),
            FieldValue::Range(v) => write!(f, "{v}"),
            FieldValue::Number(v) => write!(f, "{v}"),
        }
    }
}

/// 单帧的元数据快照。
///
/// `original` 是从码流读到的值，`current` 是修复后的工作值。
/// 缺字段用 `None` 表示。
#[derive(Debug, Clone, Default)]
pub struct FrameMeta {
    pub frame_no: u64,
    pub original: Vec<(FieldKey, Option<FieldValue>)>,
    pub current: Vec<(FieldKey, Option<FieldValue>)>,
}

impl FrameMeta {
    pub fn new(frame_no: u64) -> Self {
        let fields: Vec<(FieldKey, Option<FieldValue>)> =
            FieldKey::ALL.iter().map(|k| (*k, None)).collect();
        FrameMeta {
            frame_no,
            original: fields.clone(),
            current: fields,
        }
    }

    pub fn get(&self, key: FieldKey) -> Option<FieldValue> {
        self.current
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| *v)
    }

    pub fn get_original(&self, key: FieldKey) -> Option<FieldValue> {
        self.original
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| *v)
    }

    /// 同时写入原始值与当前值（采集阶段使用）。
    pub fn set(&mut self, key: FieldKey, value: FieldValue) {
        self.set_slot(key, Some(value), true);
        self.set_slot(key, Some(value), false);
    }

    /// 只修改当前值（修复阶段使用）。
    pub fn set_current(&mut self, key: FieldKey, value: Option<FieldValue>) {
        self.set_slot(key, value, false);
    }

    fn set_slot(&mut self, key: FieldKey, value: Option<FieldValue>, original: bool) {
        let slots = if original {
            &mut self.original
        } else {
            &mut self.current
        };
        if let Some(slot) = slots.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            slots.push((key, value));
        }
    }

    /// 当前值是否已被修复改动。
    pub fn is_modified(&self, key: FieldKey) -> bool {
        self.get(key) != self.get_original(key)
    }
}
