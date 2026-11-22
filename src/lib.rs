pub mod acceptance;
pub mod checkpoint;
pub mod compare;
pub mod demo;
pub mod evidence;
pub mod fixture;
pub mod gst_sampler;
pub mod model;
pub mod plan;
pub mod runner;
pub mod ui;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
