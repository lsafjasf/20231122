pub mod audit;
pub mod curve;
pub mod engine;
pub mod error;
pub mod model;
pub mod plan;
pub mod wav;

#[cfg(feature = "gst")]
pub mod gst_engine;

pub use audit::{load_record, verify_artifact};
pub use engine::run_with_renderer;
pub use error::AppError;
pub use model::*;
pub use plan::{build_plan, RenderPlan, RenderSegment, TimelineDraft};
