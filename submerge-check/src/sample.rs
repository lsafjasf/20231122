//! 内置示例输入：两个多语言版本快照。
//!
//! 设计要点（与处理流水线一一对应）：
//! - `snapshot_b` 第 5 条记录被截断：缺少结束时间与结束空行，文件直接结束；
//! - `snapshot_b` 第 4 条记录时间轴结构损坏（箭头缺失），触发单对象异常；
//! - B3 同时等权重叠 A2 与 A3（各 1.5s），构成无法判断的对齐边界。

/// 英文版本快照（版本 V1）。
pub fn snapshot_a_bytes() -> Vec<u8> {
    let text = concat!(
        "1\n",
        "00:00:01,000 --> 00:00:03,000\n",
        "Good morning, everyone.\n",
        "\n",
        "2\n",
        "00:00:04,000 --> 00:00:07,000\n",
        "First we review the merge plan.\n",
        "\n",
        "3\n",
        "00:00:05,500 --> 00:00:07,000\n",
        "Then we confirm the owners.\n",
        "\n",
        "4\n",
        "00:00:08,500 --> 00:00:11,000\n",
        "Please open the source file.\n",
        "\n",
        "5\n",
        "00:00:12,000 --> 00:00:14,000\n",
        "We will keep every evidence.\n",
        "\n",
    );
    text.as_bytes().to_vec()
}

/// 中文版本快照（版本 V2），含损坏记录与被截断的末条记录。
pub fn snapshot_b_bytes() -> Vec<u8> {
    let mut bytes = concat!(
        "1\n",
        "00:00:01,200 --> 00:00:03,000\n",
        "大家早上好。\n",
        "\n",
        "2\n",
        "00:00:04,000 --> 00:00:07,000\n",
        "今天我们评审合并方案。\n",
        "\n",
        "3\n",
        "00:00:05,500 --> 00:00:08,500\n",
        "先看时间轴对齐规则并确认负责人。\n",
        "\n",
        "4\n",
        "00:00:08,000 - 00:00:09,000\n",
        "请打开来源文件。\n",
        "\n",
        "5\n",
        "00:00:12,000 -->\n",
        "我们会保留全部关键证据\n",
    )
    .as_bytes()
    .to_vec();
    // 末尾追加一个被截断的多字节 UTF-8 字符（“据”的前两个字节），
    // 模拟写入过程中磁盘截断导致的非法编码，原始字节仍会作为证据保留。
    bytes.extend_from_slice(&[0xE6, 0x8D]);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_shapes_match_design() {
        let a = snapshot_a_bytes();
        let b = snapshot_b_bytes();
        assert!(a.ends_with(b"\n\n"));
        // B 以被截断的 UTF-8 序列结尾，整体不是合法 UTF-8。
        assert!(std::str::from_utf8(&b).is_err());
        assert_eq!(&b[b.len() - 2..], &[0xE6, 0x8D]);
        assert!(String::from_utf8_lossy(&b).contains("00:00:12,000 -->\n"));
    }
}
