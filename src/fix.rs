//! 修复会话：逐字段应用建议、撤销，并跟踪每次修改的依据。

use crate::meta::*;
use crate::rules::{self, Issue, Suggestion};

/// 一次已应用的修改（用于撤销与报告）。
#[derive(Debug, Clone)]
pub struct Edit {
    pub frame_idx: usize,
    pub field: FieldKey,
    pub before: Option<FieldValue>,
    pub after: FieldValue,
    pub rationale: String,
}

/// 一组帧的修复工作区。
#[derive(Debug, Default)]
pub struct RepairSession {
    pub frames: Vec<FrameMeta>,
    history: Vec<Edit>,
}

impl RepairSession {
    pub fn new(frames: Vec<FrameMeta>) -> Self {
        RepairSession {
            frames,
            history: Vec::new(),
        }
    }

    /// 重新分析某帧当前值，得到尚未解决的问题。
    pub fn issues(&self, frame_idx: usize) -> Vec<Issue> {
        rules::analyze(&self.frames[frame_idx])
    }

    /// 应用一条建议到指定帧，返回是否产生变化。
    pub fn apply(&mut self, frame_idx: usize, suggestion: &Suggestion) -> bool {
        let frame = &mut self.frames[frame_idx];
        let before = frame.get(suggestion.field);
        if before == Some(suggestion.value) {
            return false;
        }
        frame.set_current(suggestion.field, Some(suggestion.value));
        self.history.push(Edit {
            frame_idx,
            field: suggestion.field,
            before,
            after: suggestion.value,
            rationale: suggestion.rationale.clone(),
        });
        true
    }

    /// 撤销指定帧指定字段的最近一次修改。
    pub fn undo_field(&mut self, frame_idx: usize, field: FieldKey) -> bool {
        let pos = self
            .history
            .iter()
            .rposition(|e| e.frame_idx == frame_idx && e.field == field);
        let Some(pos) = pos else { return false };
        let edit = self.history.remove(pos);
        self.frames[edit.frame_idx].set_current(edit.field, edit.before);
        true
    }

    /// 撤销全局最近一次修改。
    pub fn undo_last(&mut self) -> bool {
        let Some(edit) = self.history.pop() else {
            return false;
        };
        self.frames[edit.frame_idx].set_current(edit.field, edit.before);
        true
    }

    /// 已应用的修改记录（按应用顺序）。
    pub fn edits(&self) -> &[Edit] {
        &self.history
    }

    /// 全片是否仍有未解决的问题。
    pub fn remaining_issues(&self) -> usize {
        self.frames
            .iter()
            .map(|f| rules::analyze(f).len())
            .sum()
    }
}
