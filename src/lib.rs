pub mod evidence;
pub mod experiment;
pub mod fixture;
pub mod model;
pub mod replay;

#[cfg(feature = "desktop")]
pub mod media;
#[cfg(feature = "desktop")]
pub mod ui;

pub const SCHEMA: &str = "media-meta-replay/v1";
