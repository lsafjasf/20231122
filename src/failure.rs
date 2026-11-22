use std::fmt;
use std::io;
use std::path::Path;

pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub action: String,
    pub detail: Option<String>,
}

impl Failure {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            action: action.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn io(context: &str, path: &std::path::Path, error: io::Error) -> Self {
        let action = if error.kind() == io::ErrorKind::NotFound {
            format!(
                "确认路径 `{}` 存在且可读；若清单中的旧路径已失效，用 `alc-review assess --manifest <清单> --audio <实际重放文件>` 覆盖后重试。",
                path.display()
            )
        } else if error.kind() == io::ErrorKind::PermissionDenied {
            format!("修正文件权限后重试：chmod u+r {}", path.display())
        } else {
            "检查磁盘空间、挂载状态和文件权限，然后重新运行评估。".to_string()
        };
        Self::new("io", format!("{context}失败：{}", path.display()), action)
            .with_detail(error.to_string())
    }

    pub fn json(context: &str, path: &std::path::Path, error: serde_json::Error) -> Self {
        Self::new(
            "manifest_json",
            format!("{context}解析失败：{}", path.display()),
            format!(
                "修复 JSON（第 {} 行、第 {} 列）后重试；不要删除原始清单，运行日志会保留它。",
                error.line(),
                error.column()
            ),
        )
        .with_detail(error.to_string())
    }

    pub fn missing_file(kind: &str, path: &Path, hint: &str) -> Self {
        Self::new(
            "missing_input",
            format!("{kind}不存在：{}", path.display()),
            hint,
        )
    }

    pub fn gst(operation: &str, error: impl fmt::Display) -> Self {
        let detail = error.to_string();
        let action = if detail.contains("no element")
            || detail.contains("not found")
            || detail.contains("missing")
        {
            "安装 GStreamer base/good 插件包，然后运行 `gst-inspect-1.0 decodebin` 与 `gst-inspect-1.0 wavparse` 自检。"
                .to_string()
        } else if detail.contains("Internal data stream error") {
            "用 `gst-launch-1.0 filesrc location=<文件> ! decodebin ! fakesink` 定位解码问题；必要时先转成 WAV/FLAC 再重试。"
                .to_string()
        } else {
            "运行 `GST_DEBUG=2 alc-review assess ...` 获取详细日志；确认音频文件未截断且解码插件可用。"
                .to_string()
        };
        Self::new("gstreamer", format!("{operation}失败"), action).with_detail(detail)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}。建议：{}", self.message, self.action)?;
        if let Some(detail) = &self.detail {
            write!(f, "（{detail}）")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(value: io::Error) -> Self {
        Self::new("io", "文件操作失败", "检查路径、权限与磁盘空间后重试。")
            .with_detail(value.to_string())
    }
}
