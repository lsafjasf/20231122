use anyhow::{anyhow, Result};
use gstreamer as gst;

pub fn ensure_gst() -> Result<()> {
    gst::init().map_err(|error| anyhow!("cannot initialize GStreamer: {error}"))
}

pub fn error_from_bus(bus: &gst::Bus, timeout: gst::ClockTime, context: &str) -> Result<()> {
    let Some(message) =
        bus.timed_pop_filtered(timeout, &[gst::MessageType::Error, gst::MessageType::Eos])
    else {
        return Ok(());
    };
    match message.view() {
        gst::MessageView::Error(error) => Err(anyhow!(
            "{context}: {} ({})",
            error.error(),
            error
                .debug()
                .map(|debug| debug.to_string())
                .unwrap_or_default()
        )),
        gst::MessageView::Eos(_) => Ok(()),
        _ => Ok(()),
    }
}
