use thiserror::Error;

use crate::model::{ActionKind, FailureRecord, FollowUpAction};

#[derive(Debug, Clone, Error)]
#[error("{title}: {detail}")]
pub struct AnalysisFailure {
    pub code: String,
    pub title: String,
    pub detail: String,
    pub retryable: bool,
    pub actions: Vec<FollowUpAction>,
}

impl AnalysisFailure {
    pub fn new(code: &str, title: &str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let actions = default_actions(code, &detail);
        Self {
            code: code.into(),
            title: title.into(),
            detail,
            retryable: true,
            actions,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_action(mut self, action: FollowUpAction) -> Self {
        if !self.actions.iter().any(|item| item.code == action.code) {
            self.actions.push(action);
        }
        self
    }

    pub fn to_record(&self, evidence_retained: bool) -> FailureRecord {
        FailureRecord {
            code: self.code.clone(),
            title: self.title.clone(),
            detail: self.detail.clone(),
            retryable: self.retryable,
            evidence_retained,
            actions: self.actions.clone(),
        }
    }

    pub fn missing_source(path: &str, detail: impl Into<String>) -> Self {
        Self::new("missing_source", "源文件不可访问", detail).with_action(FollowUpAction {
            code: "locate_source".into(),
            label: "重新选择源文件".into(),
            detail: format!("确认 {path} 仍在原位置，或从恢复备份重新选择同一文件。"),
            kind: ActionKind::ChooseFile,
            command: None,
            target_path: Some(path.into()),
        })
    }

    pub fn missing_decoder(detail: impl Into<String>) -> Self {
        Self::new("missing_decoder", "缺少解码器或 GStreamer 插件", detail)
            .with_action(FollowUpAction {
                code: "inspect_decoder".into(),
                label: "检查 decodebin 与插件清单".into(),
                detail: "找到缺失元素后安装对应插件包，再重新重放。".into(),
                kind: ActionKind::CopyCommand,
                command: Some(
                    "gst-inspect-1.0 decodebin; gst-inspect-1.0 videoconvert; gst-inspect-1.0 videorate"
                        .into(),
                ),
                target_path: None,
            })
            .with_action(FollowUpAction {
                code: "probe_source".into(),
                label: "用 gst-discoverer 查看容器与编码".into(),
                detail: "确认文件实际容器/编码；未知扩展名不代表格式不可读。".into(),
                kind: ActionKind::CopyCommand,
                command: Some("gst-discoverer-1.0 -v <source-file>".into()),
                target_path: None,
            })
    }

    pub fn unsupported_media(detail: impl Into<String>) -> Self {
        Self::new("unsupported_media", "媒体内容无法解析", detail).with_action(FollowUpAction {
            code: "probe_content".into(),
            label: "按内容而非扩展名探测".into(),
            detail: "未知扩展名会被保留为证据；请确认 decodebin 探测出的真实 caps。".into(),
            kind: ActionKind::CopyCommand,
            command: Some(
                "gst-typefind-1.0 <source-file>; gst-discoverer-1.0 -v <source-file>".into(),
            ),
            target_path: None,
        })
    }

    pub fn no_video_track(detail: impl Into<String>) -> Self {
        Self::new("no_video_track", "没有找到可用视频轨", detail).with_action(FollowUpAction {
            code: "list_tracks".into(),
            label: "列出轨道并切换轨道索引".into(),
            detail: "在轨道选择器中改用其他索引后重放；音频或字幕轨不能做亮度黑场检测。".into(),
            kind: ActionKind::Guidance,
            command: Some("gst-discoverer-1.0 -v <source-file>".into()),
            target_path: None,
        })
    }

    pub fn no_samples(detail: impl Into<String>) -> Self {
        Self::new("no_video_samples", "解码成功但没有亮度波形", detail)
            .with_action(FollowUpAction {
                code: "retry_lower_rate".into(),
                label: "降低采样帧率后重试".into(),
                detail: "把采样帧率改为 2 fps，并确认输出像素格式为 GRAY8。".into(),
                kind: ActionKind::Retry,
                command: Some(
                    "gst-launch-1.0 filesrc location=<source-file> ! decodebin ! videoconvert ! video/x-raw,format=GRAY8 ! fakesink"
                        .into(),
                ),
                target_path: None,
            })
    }

    pub fn output_failure(path: &str, detail: impl Into<String>) -> Self {
        Self::new("evidence_write_failed", "证据写入失败", detail).with_action(FollowUpAction {
            code: "choose_output".into(),
            label: "更换可写目录".into(),
            detail: "选择空间充足且可写的目录；不要覆盖上一次证据包。".into(),
            kind: ActionKind::OpenPath,
            command: Some(format!("ls -ld -- {path} && df -h -- {path}")),
            target_path: Some(path.into()),
        })
    }

    pub fn integrity_mismatch(detail: impl Into<String>) -> Self {
        Self::new("integrity_mismatch", "源文件指纹不一致", detail)
            .retryable(false)
            .with_action(FollowUpAction {
                code: "restore_source".into(),
                label: "从恢复副本取回原文件".into(),
                detail: "不要沿用旧报告的 SHA-256；以恢复副本重新计算并保留两份指纹。".into(),
                kind: ActionKind::Guidance,
                command: Some("sha256sum <recovered-source>".into()),
                target_path: None,
            })
    }
}

fn default_actions(code: &str, detail: &str) -> Vec<FollowUpAction> {
    match code {
        "cancelled" => vec![FollowUpAction {
            code: "restart_replay".into(),
            label: "重新开始重放".into(),
            detail: "已有波形与事件日志会保留，重新运行会生成新的会话。".into(),
            kind: ActionKind::Retry,
            command: None,
            target_path: None,
        }],
        "permission_denied" => vec![FollowUpAction {
            code: "fix_permissions".into(),
            label: "检查读取权限".into(),
            detail: "确认当前用户对源文件及证据目录有读写权限。".into(),
            kind: ActionKind::OpenPath,
            command: Some("ls -l <source-file>; df -h <evidence-dir>".into()),
            target_path: None,
        }],
        _ => vec![FollowUpAction {
            code: "retry_same_session".into(),
            label: "重试当前重放".into(),
            detail: format!("保留会话与已产生的证据后重试。原始错误：{detail}"),
            kind: ActionKind::Retry,
            command: None,
            target_path: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActionKind;

    #[test]
    fn missing_decoder_failure_has_inspectable_commands() {
        let failure = AnalysisFailure::missing_decoder("decodebin unavailable");
        assert_eq!(failure.code, "missing_decoder");
        assert!(failure.actions.iter().any(|action| {
            action.kind == ActionKind::CopyCommand
                && action
                    .command
                    .as_deref()
                    .unwrap_or_default()
                    .contains("gst-inspect-1.0")
        }));
        let record = failure.to_record(false);
        assert!(!record.evidence_retained);
        assert_eq!(record.actions.len(), failure.actions.len());
    }

    #[test]
    fn missing_source_offers_file_selection() {
        let failure = AnalysisFailure::missing_source("/missing.recx", "not found");
        assert!(failure
            .actions
            .iter()
            .any(|action| action.kind == ActionKind::ChooseFile));
    }
}
