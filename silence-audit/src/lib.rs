//! 音轨静音段识别可复核流程：快照解析、GStreamer 检测、GTK4 复核与证据报告。

pub use gstreamer as gst;
pub use gstreamer_audio as gst_audio;
pub use gtk4 as gtk;

pub mod model;
pub mod pipeline;
pub mod report;
pub mod runner;
pub mod snapshot;
pub mod ui;
