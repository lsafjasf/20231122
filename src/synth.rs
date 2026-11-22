//! 合成帧源：生成正常 / 缺字段 / 互相冲突三类验证帧。
//!
//! 既供单元测试与自检使用，也供 GUI 的"加载示例帧"按钮使用，
//! 这样没有真实 HDR 素材时也能完整走通修复流程。

use crate::meta::*;

fn hdr10_baseline(frame_no: u64) -> FrameMeta {
    let mut f = FrameMeta::new(frame_no);
    f.set(FieldKey::HdrFormat, FieldValue::Format(HdrFormat::Hdr10));
    f.set(FieldKey::Transfer, FieldValue::Transfer(Transfer::Pq));
    f.set(FieldKey::Primaries, FieldValue::Primaries(Primaries::Bt2020));
    f.set(FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt2020Ncl));
    f.set(FieldKey::Range, FieldValue::Range(Range::Limited));
    f.set(FieldKey::MasteringMaxLum, FieldValue::Number(1000.0));
    f.set(FieldKey::MasteringMinLum, FieldValue::Number(0.0001));
    f.set(FieldKey::MaxCll, FieldValue::Number(800.0));
    f.set(FieldKey::MaxFall, FieldValue::Number(400.0));
    f.set(FieldKey::ExposureValue, FieldValue::Number(9.0));
    f
}

/// 正常帧：HDR10 与 HLG 各一，应当零问题。
pub fn normal_frames() -> Vec<FrameMeta> {
    let hlg = {
        let mut f = FrameMeta::new(1);
        f.set(FieldKey::HdrFormat, FieldValue::Format(HdrFormat::Hlg));
        f.set(FieldKey::Transfer, FieldValue::Transfer(Transfer::Hlg));
        f.set(FieldKey::Primaries, FieldValue::Primaries(Primaries::Bt2020));
        f.set(FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt2020Ncl));
        f.set(FieldKey::Range, FieldValue::Range(Range::Limited));
        f
    };
    vec![hdr10_baseline(0), hlg]
}

/// 缺字段帧：声明 HDR10 但缺静态元数据与色彩信息。
pub fn missing_field_frames() -> Vec<FrameMeta> {
    let mut a = FrameMeta::new(10);
    a.set(FieldKey::HdrFormat, FieldValue::Format(HdrFormat::Hdr10));
    a.set(FieldKey::Transfer, FieldValue::Transfer(Transfer::Pq));
    // 缺 primaries/matrix/range/mastering/cll/fall

    let mut b = FrameMeta::new(11);
    b.set(FieldKey::Transfer, FieldValue::Transfer(Transfer::Pq));
    b.set(FieldKey::Primaries, FieldValue::Primaries(Primaries::Bt2020));
    // 缺 HDR 格式标签
    vec![a, b]
}

/// 互相冲突的帧：标签/传递函数/矩阵/亮度/曝光彼此矛盾。
pub fn conflicting_frames() -> Vec<FrameMeta> {
    // 帧 20：HDR10 标签 + HLG 传递函数 + bt709 矩阵。
    let mut a = hdr10_baseline(20);
    a.set(FieldKey::Transfer, FieldValue::Transfer(Transfer::Hlg));
    a.set(FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt709));

    // 帧 21：MaxFALL > MaxCLL，MaxCLL > 母版峰值。
    let mut b = hdr10_baseline(21);
    b.set(FieldKey::MaxCll, FieldValue::Number(1500.0));
    b.set(FieldKey::MaxFall, FieldValue::Number(2000.0));

    // 帧 22：EV 隐含亮度远超母版峰值；BT.2020 CL + full range。
    let mut c = hdr10_baseline(22);
    c.set(FieldKey::ExposureValue, FieldValue::Number(14.0));
    c.set(FieldKey::Matrix, FieldValue::Matrix(Matrix::Bt2020Cl));
    c.set(FieldKey::Range, FieldValue::Range(Range::Full));

    // 帧 23：SDR 标签 + PQ 传递函数。
    let mut d = hdr10_baseline(23);
    d.set(FieldKey::HdrFormat, FieldValue::Format(HdrFormat::Sdr));
    vec![a, b, c, d]
}

/// 全部示例帧（供 GUI 一键加载）。
pub fn sample_frames() -> Vec<FrameMeta> {
    let mut v = normal_frames();
    v.extend(missing_field_frames());
    v.extend(conflicting_frames());
    v
}
