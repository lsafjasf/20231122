use crate::model::{BlockStyle, CreditsBlock, CreditsSample};

fn block(id: &str, style: BlockStyle, lines: &[&str]) -> CreditsBlock {
    CreditsBlock {
        id: id.to_string(),
        style,
        hard_lines: lines.iter().map(|line| (*line).to_string()).collect(),
    }
}

pub fn all_samples() -> Vec<CreditsSample> {
    vec![mixed_sample(), emoji_sample(), long_line_sample()]
}

pub fn sample_by_id(id: &str) -> Option<CreditsSample> {
    all_samples().into_iter().find(|sample| sample.id == id)
}

pub fn mixed_sample() -> CreditsSample {
    use BlockStyle::*;
    CreditsSample {
        id: "mixed-cjk-english".into(),
        label: "中英文混排 / Mixed CJK + English".into(),
        blocks: vec![
            block("B00", Title, &["片尾字幕 / End Credits"]),
            block("B01", Body, &[""]),
            block("B02", Heading, &["导演 Director"]),
            block("B03", Body, &["林亦菲 Lin Yifei（中国 / United Kingdom）"]),
            block("B04", Heading, &["主演 Cast"]),
            block("B05", Body, &["陈默 Chen Mo 饰 调查记者 Investigative Journalist"]),
            block("B06", Body, &["Ava Müller 阿娃·穆勒 饰 数据工程师 Data Engineer"]),
            block("B07", Heading, &["特别鸣谢 Special Thanks"]),
            block(
                "B08",
                Body,
                &[
                    "感谢开源社区的志愿者们，以及所有参与试映的观众。",
                    "Thanks to every open-source volunteer and every test-screening viewer.",
                ],
            ),
            block("B09", Body, &[""]),
            block(
                "B10",
                Caption,
                &["© 2026 示例工作室 Example Studio — 保留所有权利 All rights reserved"],
            ),
        ],
    }
}

pub fn emoji_sample() -> CreditsSample {
    use BlockStyle::*;
    CreditsSample {
        id: "emoji-heavy".into(),
        label: "表情符号 / Emoji Heavy".into(),
        blocks: vec![
            block("B00", Title, &["🎉 制作特辑 Featurette 🎬"]),
            block(
                "B01",
                Body,
                &["👩‍💻 研发团队 Engineering：Rust 🦀 + GTK4 🪟 + GStreamer 🎞️"],
            ),
            block(
                "B02",
                Body,
                &["本地化 Localization：简体中文 🇨🇳、English 🇬🇧、日本語 🇯🇵、한국어 🇰🇷"],
            ),
            block("B03", Heading, &["评分 Rating：⭐⭐⭐⭐⭐ 5.0/5.0"]),
            block(
                "B04",
                Body,
                &["状态 Status：✅ 已通过安全区检查 Safe-area verified　⚠️ 请核对字幕轴"],
            ),
            block("B05", Heading, &["❤️ 感谢观看 Thanks for watching ❤️"]),
        ],
    }
}

pub fn long_line_sample() -> CreditsSample {
    use BlockStyle::*;
    let repeated = "https://credits.example.org/".repeat(18);
    let long = format!(
        "{repeated}supercalifragilisticexpialidocious-credits-line-that-must-not-silently-overflow"
    );
    CreditsSample {
        id: "super-long-line".into(),
        label: "超长单行 / Unbroken Long Line".into(),
        blocks: vec![
            block("B00", Title, &["超长单行压力测试 Ultra-long Single Line"]),
            block("B01", Body, &[long.as_str()]),
            block(
                "B02",
                Body,
                &["该行没有任何空格或断点，用于验证字符级折行与逐行越界记录是否生效。"],
            ),
        ],
    }
}
