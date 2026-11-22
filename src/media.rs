use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

use gstreamer as gst;
use gstreamer_pbutils as pbutils;
use serde::{Deserialize, Serialize};

use crate::SCHEMA;
use crate::evidence::{sha256_bytes, sha256_json};

#[derive(Debug)]
pub struct MediaProbeError(String);

impl MediaProbeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MediaProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MediaProbeError {}

impl From<std::io::Error> for MediaProbeError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<gst::glib::Error> for MediaProbeError {
    fn from(error: gst::glib::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioProbe {
    pub bitrate: u32,
    pub channels: u32,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaFacts {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub duration_ms: Option<u64>,
    pub is_seekable: bool,
    pub discoverer_result: String,
    pub fields: BTreeMap<String, String>,
    pub audio: Option<AudioProbe>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProbe {
    pub schema: String,
    pub facts: MediaFacts,
    pub gstreamer_version: String,
    pub probe_sha256: String,
}

pub fn probe_media(path: &Path) -> Result<MediaProbe, MediaProbeError> {
    gst::init()?;
    let canonical = path.canonicalize().map_err(|error| {
        MediaProbeError::new(format!("cannot resolve {}: {error}", path.display()))
    })?;
    let bytes = fs::read(&canonical)?;
    let uri = gst::glib::filename_to_uri(&canonical, None)?;
    let discoverer = pbutils::Discoverer::new(gst::ClockTime::from_seconds(10))?;
    let info = discoverer.discover_uri(uri.as_str())?;
    let result = info.result();
    if result != pbutils::DiscovererResult::Ok {
        return Err(MediaProbeError::new(format!(
            "GStreamer Discoverer failed for {}: {result:?}",
            canonical.display()
        )));
    }

    let tags = info.tags();
    let mut fields = BTreeMap::new();
    insert_tag(&mut fields, "title", tags.as_ref(), tag_title);
    insert_tag(&mut fields, "artist", tags.as_ref(), tag_artist);
    insert_tag(&mut fields, "album", tags.as_ref(), tag_album);
    insert_tag(
        &mut fields,
        "container_format",
        tags.as_ref(),
        tag_container_format,
    );
    insert_tag(&mut fields, "audio_codec", tags.as_ref(), tag_audio_codec);
    if let Some(duration) = info.duration() {
        fields.insert("duration_ms".to_owned(), duration.mseconds().to_string());
    }

    let audio = info
        .audio_streams()
        .into_iter()
        .next()
        .map(|stream| AudioProbe {
            bitrate: stream.bitrate(),
            channels: stream.channels(),
            sample_rate: stream.sample_rate(),
        });
    if let Some(audio) = &audio {
        fields.insert("channels".to_owned(), audio.channels.to_string());
        fields.insert("sample_rate".to_owned(), audio.sample_rate.to_string());
    }

    let facts = MediaFacts {
        path: path.display().to_string(),
        size_bytes: bytes.len() as u64,
        sha256: sha256_bytes(&bytes),
        duration_ms: info.duration().map(|duration| duration.mseconds()),
        is_seekable: info.is_seekable(),
        discoverer_result: format!("{result:?}"),
        fields,
        audio,
    };
    let probe_sha256 = sha256_json(&facts);
    Ok(MediaProbe {
        schema: SCHEMA.to_owned(),
        facts,
        gstreamer_version: gst::version_string().to_string(),
        probe_sha256,
    })
}

fn insert_tag(
    fields: &mut BTreeMap<String, String>,
    key: &str,
    tags: Option<&gst::TagList>,
    read: fn(&gst::TagList) -> Option<String>,
) {
    if let Some(value) = tags.and_then(read) {
        fields.insert(key.to_owned(), value);
    }
}

fn tag_title(tags: &gst::TagList) -> Option<String> {
    tags.get::<gst::tags::Title>()
        .map(|value| value.get().to_owned())
}

fn tag_artist(tags: &gst::TagList) -> Option<String> {
    tags.get::<gst::tags::Artist>()
        .map(|value| value.get().to_owned())
}

fn tag_album(tags: &gst::TagList) -> Option<String> {
    tags.get::<gst::tags::Album>()
        .map(|value| value.get().to_owned())
}

fn tag_container_format(tags: &gst::TagList) -> Option<String> {
    tags.get::<gst::tags::ContainerFormat>()
        .map(|value| value.get().to_owned())
}

fn tag_audio_codec(tags: &gst::TagList) -> Option<String> {
    tags.get::<gst::tags::AudioCodec>()
        .map(|value| value.get().to_owned())
}
