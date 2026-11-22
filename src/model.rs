use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyState {
    Missing,
    Ready,
    Invalid,
    Recovering,
    Quarantined,
}

impl ProxyState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyState::Missing => "missing",
            ProxyState::Ready => "ready",
            ProxyState::Invalid => "invalid",
            ProxyState::Recovering => "recovering",
            ProxyState::Quarantined => "quarantined",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxySpec {
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub encoder: String,
}

impl Default for ProxySpec {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
            pixel_format: "RGB".into(),
            encoder: "pngenc".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMeta {
    pub schema_version: u32,
    pub asset_id: String,
    pub source_path: String,
    pub source_extension: String,
    pub source_sha256: String,
    pub source_size: u64,
    pub proxy_path: String,
    pub proxy_sha256: Option<String>,
    pub proxy_size: Option<u64>,
    pub decode_fingerprint: Option<String>,
    pub proxy_spec: ProxySpec,
    pub state: ProxyState,
    pub generation: u32,
    pub last_action: String,
    pub last_error: Option<String>,
    pub updated_at_unix_ms: u128,
}

impl AssetMeta {
    pub fn new(
        asset_id: &str,
        source_path: PathBuf,
        proxy_path: PathBuf,
        source_sha256: String,
        source_size: u64,
        spec: ProxySpec,
    ) -> Self {
        let source_extension = extension_of(&source_path);
        Self {
            schema_version: 1,
            asset_id: asset_id.into(),
            source_path: source_path.to_string_lossy().into_owned(),
            source_extension,
            source_sha256,
            source_size,
            proxy_path: proxy_path.to_string_lossy().into_owned(),
            proxy_sha256: None,
            proxy_size: None,
            decode_fingerprint: None,
            proxy_spec: spec,
            state: ProxyState::Missing,
            generation: 0,
            last_action: "created".into(),
            last_error: None,
            updated_at_unix_ms: crate::hash::now_unix_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaFingerprint {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub frames: u64,
    pub raw_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyValidation {
    pub exists: bool,
    pub size_matches: bool,
    pub hash_matches: bool,
    pub decodes: bool,
    pub dimensions_match: bool,
    pub actual_sha256: Option<String>,
    pub actual_size: Option<u64>,
    pub fingerprint: Option<MediaFingerprint>,
    pub reasons: Vec<String>,
}

impl ProxyValidation {
    pub fn invalid() -> Self {
        Self {
            exists: false,
            size_matches: false,
            hash_matches: false,
            decodes: false,
            dimensions_match: false,
            actual_sha256: None,
            actual_size: None,
            fingerprint: None,
            reasons: vec!["proxy_missing".into()],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.exists
            && self.size_matches
            && self.hash_matches
            && self.decodes
            && self.dimensions_match
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

pub fn extension_of(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn extension_supported(path: &std::path::Path) -> bool {
    SUPPORTED_EXTENSIONS.contains(&extension_of(path).as_str())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseOutcome {
    pub asset_id: String,
    pub decision: String,
    pub cache_hit: bool,
    pub regenerated: bool,
    pub generation_before: u32,
    pub generation_after: u32,
    pub state_before: String,
    pub state_after: String,
    pub transitions: Vec<String>,
    pub proxy_sha256: Option<String>,
    pub decode_fingerprint: Option<String>,
    pub proxy_file_unchanged: bool,
    pub reason: String,
}
