use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 安全框检测处理参数。同一份参数决定同一条输出波形，
/// 记录与重放都通过 `hash()` 绑定到具体参数。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafeFrameParams {
    /// 动作安全框边占比（如 0.93）
    pub action_safe: f64,
    /// 字幕安全框边占比（如 0.90）
    pub title_safe: f64,
    /// 判定使用的安全框: "action" | "title"
    pub mode: String,
    /// 活跃像素亮度差阈值
    pub luma_threshold: u8,
    /// 安全框外活跃像素占比超过该值判为违规
    pub max_ratio: f64,
    /// 像素采样步长（性能/精度折中）
    pub sample_step: u32,
}

impl Default for SafeFrameParams {
    fn default() -> Self {
        Self {
            action_safe: 0.93,
            title_safe: 0.90,
            mode: "title".into(),
            luma_threshold: 48,
            max_ratio: 0.001,
            sample_step: 2,
        }
    }
}

impl SafeFrameParams {
    pub fn safe_fraction(&self) -> f64 {
        match self.mode.as_str() {
            "action" => self.action_safe,
            _ => self.title_safe,
        }
    }

    /// 参数指纹：记录头与每条 entry 都携带，重放时据此校验一致性。
    pub fn hash(&self) -> String {
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}",
            self.action_safe,
            self.title_safe,
            self.mode,
            self.luma_threshold,
            self.max_ratio,
            self.sample_step
        );
        let mut h = DefaultHasher::new();
        canonical.hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// 安全框矩形（像素），返回 (x, y, w, h)
    pub fn safe_rect(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let f = self.safe_fraction();
        let w = (width as f64 * f).round() as u32;
        let h = (height as f64 * f).round() as u32;
        ((width - w) / 2, (height - h) / 2, w, h)
    }
}
