//! Application service layer.
//!
//! One place that owns state transitions, audit entries and persistence so the
//! CLI and GTK UI stay thin and behave identically on reopen.

use std::path::{Path, PathBuf};

use crate::compare::compare_segments;
use crate::extract::{extract_segment, CancellationToken, ExtractionOutcome, Progress};
use crate::media::inspect_media;
use crate::model::project::{
    Device, ExtractionConfig, KeyframeMode, MediaSegment, Project, SegmentStatus,
};
use crate::storage::ProjectStore;
use crate::time::ClockOffset;
use crate::{Error, Result};

/// Reusable facade over one opened project.
pub struct ProjectService {
    pub store: ProjectStore,
    pub project: Project,
    pub actor: String,
}

impl ProjectService {
    pub fn open(root: impl Into<PathBuf>, actor: impl Into<String>) -> Result<Self> {
        let store = ProjectStore::open(root)?;
        let project = store.load()?;
        Ok(Self {
            store,
            project,
            actor: actor.into(),
        })
    }

    pub fn create(
        root: impl Into<PathBuf>,
        name: &str,
        description: &str,
        actor: &str,
    ) -> Result<Self> {
        let (store, project) = ProjectStore::create(root, name, description, actor)?;
        Ok(Self {
            store,
            project,
            actor: actor.to_string(),
        })
    }

    pub fn save(&mut self) -> Result<()> {
        self.project.touch();
        self.store.save(&self.project)
    }

    pub fn add_device(&mut self, label: &str, notes: &str) -> Result<Device> {
        let mut device = Device::new(label);
        device.notes = notes.to_string();
        self.project.devices.push(device.clone());
        self.save()?;
        self.store
            .audit_global(&self.actor, "device.added", &format!("label={label}"))?;
        Ok(device)
    }

    pub fn set_device_offset(
        &mut self,
        device_id: &str,
        offset_ns: i64,
        uncertainty_ns: i64,
    ) -> Result<()> {
        let device = self
            .project
            .devices
            .iter_mut()
            .find(|candidate| candidate.id == device_id)
            .ok_or_else(|| Error::DeviceUnknown(device_id.to_string()))?;
        let previous = device.clock_offset.offset_ns;
        device.clock_offset = ClockOffset {
            offset_ns,
            uncertainty_ns: uncertainty_ns.max(0),
        };
        self.save()?;
        self.store.audit_global(
            &self.actor,
            "device.offset_changed",
            &format!("device={device_id} previous_ns={previous} new_ns={offset_ns}"),
        )?;
        Ok(())
    }

    /// Register a fragment: hash + probe the media, create the segment in
    /// `pending` state, persist and audit. Nothing is decoded yet.
    pub fn register_segment(
        &mut self,
        device_id: &str,
        label: &str,
        source_path: &Path,
        clock_origin_unix_ns: i64,
        group_id: &str,
        config: ExtractionConfig,
    ) -> Result<MediaSegment> {
        if self.project.device(device_id).is_none() {
            return Err(Error::DeviceUnknown(device_id.to_string()));
        }
        let info = inspect_media(source_path)?;
        let mut segment = MediaSegment::new(
            device_id,
            label,
            source_path.to_path_buf(),
            clock_origin_unix_ns,
            group_id,
        );
        segment.source_sha256 = info.sha256.clone();
        segment.source_bytes = info.bytes;
        segment.duration_ns = info.duration_ns;
        segment.config = config;
        self.project.segments.push(segment.clone());
        self.save()?;

        let mut audit = self.store.audit_for(&segment.id)?;
        audit.append(
            &self.actor,
            "segment.registered",
            &format!(
                "label={label} source={} sha256={} duration_ns={}",
                source_path.display(),
                info.sha256,
                info.duration_ns.unwrap_or(-1)
            ),
            None,
        )?;
        Ok(segment)
    }

    pub fn configure_segment(&mut self, segment_id: &str, config: ExtractionConfig) -> Result<()> {
        let segment = self
            .project
            .segment_mut(segment_id)
            .ok_or_else(|| Error::SegmentUnknown(segment_id.to_string()))?;
        segment.config = config;
        if segment.status == SegmentStatus::Extracted {
            segment.status = SegmentStatus::Stale;
        }
        self.save()?;
        let mut audit = self.store.audit_for(segment_id)?;
        audit.append(
            &self.actor,
            "segment.config_changed",
            "extraction config updated; status -> stale if previously extracted",
            None,
        )?;
        Ok(())
    }

    pub fn mark_rerun_requested(&mut self, segment_id: &str) -> Result<()> {
        let segment = self
            .project
            .segment_mut(segment_id)
            .ok_or_else(|| Error::SegmentUnknown(segment_id.to_string()))?;
        segment.status = SegmentStatus::Stale;
        self.save()?;
        let mut audit = self.store.audit_for(segment_id)?;
        audit.append(&self.actor, "segment.rerun_requested", "", None)?;
        Ok(())
    }

    fn offset_for(&self, segment: &MediaSegment) -> ClockOffset {
        self.project
            .device(&segment.device_id)
            .map(|device| device.clock_offset)
            .unwrap_or_else(ClockOffset::zero)
    }

    /// Run (or re-run) one fragment in isolation. Cancellation leaves partial
    /// evidence on disk and an `interrupted` status, ready to resume later.
    pub fn run_segment(
        &mut self,
        segment_id: &str,
        cancel: &CancellationToken,
        progress: impl Fn(Progress),
    ) -> Result<ExtractionOutcome> {
        let offset = {
            let segment = self
                .project
                .segment(segment_id)
                .ok_or_else(|| Error::SegmentUnknown(segment_id.to_string()))?;
            if !segment.is_runnable() {
                return Err(Error::NotRunnable {
                    segment: segment_id.to_string(),
                    state: segment.status.as_str().to_string(),
                });
            }
            self.offset_for(segment)
        };
        let outcome = {
            let segment = self
                .project
                .segment_mut(segment_id)
                .ok_or_else(|| Error::SegmentUnknown(segment_id.to_string()))?;
            extract_segment(&self.store, segment, offset, &self.actor, cancel, progress)?
        };
        self.save()?;
        Ok(outcome)
    }

    pub fn compare(
        &mut self,
        segment_a_id: &str,
        segment_b_id: &str,
        match_window_ns: i64,
        notes: &str,
    ) -> Result<crate::model::project::Comparison> {
        let comparison = compare_segments(
            &mut self.project,
            segment_a_id,
            segment_b_id,
            match_window_ns,
            notes,
        )?;
        self.save()?;
        self.store.audit_global(
            &self.actor,
            "comparison.created",
            &format!(
                "comparison={} pairs={} median_delta_ns={:?}",
                comparison.id,
                comparison.entries.len(),
                comparison.median_delta_ns
            ),
        )?;
        Ok(comparison)
    }

    pub fn export(
        &self,
        export_root: &Path,
        bundle_name: &str,
    ) -> Result<crate::export::ExportReport> {
        crate::export::export_bundle(
            &self.store,
            &self.project,
            export_root,
            bundle_name,
            &self.actor,
        )
    }
}

/// Parse a mode string used by CLI/UI selectors.
pub fn parse_mode(text: &str) -> Result<KeyframeMode> {
    match text.trim().to_ascii_lowercase().as_str() {
        "interval" | "uniform" => Ok(KeyframeMode::Interval),
        "keyframe" | "keyframe_only" | "idr" => Ok(KeyframeMode::KeyframeOnly),
        other => Err(Error::Validation(format!("unknown keyframe mode: {other}"))),
    }
}
