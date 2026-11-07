//! Rust port of Chris Le's MIT-licensed virtualdj-connect.
mod history;
mod network;
mod runtime;
pub use history::*;
pub use network::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPayload {
    pub title: String,
    pub artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deck: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_on_air: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    pub file_location: String,
    pub is_beatport_stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beatport_id: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Ready {
        #[serde(rename = "basePath")]
        base_path: String,
    },
    Track {
        track: Box<TrackPayload>,
    },
    Error {
        message: String,
    },
    Sandbox {
        active: bool,
    },
    OnAir {
        active: bool,
        deck: u8,
    },
}
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
pub trait Logger: Send + Sync {
    fn log(&self, level: LogLevel, message: &str);
}
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopLogger;
impl Logger for NoopLogger {
    fn log(&self, _level: LogLevel, _message: &str) {}
}
pub(crate) fn some(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.into())
    }
}
pub fn detect_beatport(location: &str) -> Option<u64> {
    let start = location.find("netsearch://bp")? + "netsearch://bp".len();
    let digits = location[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}
pub(crate) fn base_payload(title: &str, artist: &str, location: &str) -> TrackPayload {
    let beatport_id = detect_beatport(location);
    TrackPayload {
        title: if title.is_empty() {
            "Unknown Title".into()
        } else {
            title.into()
        },
        artist: if artist.is_empty() {
            "Unknown Artist".into()
        } else {
            artist.into()
        },
        file_path: if !location.is_empty() && !location.starts_with("netsearch://") {
            Some(location.into())
        } else {
            None
        },
        file_location: location.into(),
        is_beatport_stream: beatport_id.is_some(),
        beatport_id,
        ..TrackPayload::default()
    }
}

/// The source package exports this metadata shape but implements no database reader.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbTrack {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub label: Option<String>,
    pub remix: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub comment: Option<String>,
    pub file_path: Option<String>,
}
