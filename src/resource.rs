use crate::model::{AnalysisQuality, CameraProfile, ResourcePlan};

const FIXED_OVERHEAD_BYTES: u64 = 32 * 1024;

pub fn plan_resource(
    profile: &CameraProfile,
    requested_chunk_frames: u32,
    budget_bytes: u64,
) -> ResourcePlan {
    let source_frame = profile.frame_bytes();
    let rgba_frame = source_frame.saturating_mul(2);
    let requested = requested_chunk_frames.max(1);

    if budget_bytes >= rgba_frame.saturating_mul(4) + FIXED_OVERHEAD_BYTES {
        return ResourcePlan {
            quality: AnalysisQuality::Full,
            chunk_frames: requested.min(8),
            pixel_step: 1,
            estimated_bytes: rgba_frame.saturating_mul(2) + FIXED_OVERHEAD_BYTES,
            budget_bytes,
            degraded_reason: None,
        };
    }

    if budget_bytes >= rgba_frame.saturating_mul(2) + FIXED_OVERHEAD_BYTES {
        return ResourcePlan {
            quality: AnalysisQuality::Full,
            chunk_frames: requested.min(2),
            pixel_step: 1,
            estimated_bytes: rgba_frame.saturating_mul(2) + FIXED_OVERHEAD_BYTES,
            budget_bytes,
            degraded_reason: Some("块大小从请求值降为 2 帧以控制峰值内存".into()),
        };
    }

    if budget_bytes >= rgba_frame + FIXED_OVERHEAD_BYTES {
        return ResourcePlan {
            quality: AnalysisQuality::Sampled { pixel_step: 2 },
            chunk_frames: 1,
            pixel_step: 2,
            estimated_bytes: rgba_frame + FIXED_OVERHEAD_BYTES,
            budget_bytes,
            degraded_reason: Some("低内存：单帧处理，亮度采样步长 2，置信度降级".into()),
        };
    }

    ResourcePlan {
        quality: AnalysisQuality::Blocked,
        chunk_frames: 0,
        pixel_step: 0,
        estimated_bytes: rgba_frame + FIXED_OVERHEAD_BYTES,
        budget_bytes,
        degraded_reason: Some(format!(
            "内存预算 {} 字节低于单帧工作集 {} 字节，停在安全边界",
            budget_bytes,
            rgba_frame + FIXED_OVERHEAD_BYTES
        )),
    }
}

pub fn minimum_budget(profile: &CameraProfile) -> u64 {
    profile.frame_bytes().saturating_mul(2) + FIXED_OVERHEAD_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::demo_profile;

    #[test]
    fn full_budget_keeps_requested_chunk() {
        let plan = plan_resource(&demo_profile(), 8, 1024 * 1024);
        assert_eq!(plan.quality, AnalysisQuality::Full);
        assert_eq!(plan.chunk_frames, 8);
        assert!(plan.degraded_reason.is_none());
    }

    #[test]
    fn shrinking_budget_walks_degrade_then_block() {
        let profile = demo_profile();
        let constrained = plan_resource(&profile, 8, 80 * 1024);
        assert_eq!(
            constrained.quality,
            AnalysisQuality::Sampled { pixel_step: 2 }
        );
        assert_eq!(constrained.chunk_frames, 1);
        assert!(constrained.degraded_reason.is_some());

        let blocked = plan_resource(&profile, 8, 40 * 1024);
        assert_eq!(blocked.quality, AnalysisQuality::Blocked);
        assert_eq!(blocked.chunk_frames, 0);
    }
}
