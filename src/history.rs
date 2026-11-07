use crate::{
    base_payload,
    runtime::{self, Control},
    some, Error, Event, Logger, TrackPayload,
};
use chrono::{DateTime, Duration as TimeDelta, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub const MAX_HISTORY_BYTES: u64 = 64 * 1024 * 1024;
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_date"
    )]
    pub time: Option<DateTime<Utc>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_date"
    )]
    pub last_play_time: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remix: Option<String>,
    pub file_location: String,
    pub id: String,
}
fn serialize_date<S: serde::Serializer>(
    value: &Option<DateTime<Utc>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => {
            serializer.serialize_str(&value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        }
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub found: bool,
    pub path: PathBuf,
    pub version: String,
    pub has_history: bool,
    pub write_history_enabled: bool,
}
pub fn default_path_for(home: &Path, platform: &str, require_database: bool) -> Option<PathBuf> {
    let first = match platform {
        "darwin" => home.join("Library/Application Support/VirtualDJ"),
        "win32" => home.join("AppData/Local/VirtualDJ"),
        _ => return None,
    };
    [first, home.join("Documents/VirtualDJ")]
        .into_iter()
        .find(|path| {
            path.join("database.xml").exists()
                || (!require_database && path.join("settings.xml").exists())
        })
}
fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}
fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "other"
    }
}
pub fn get_default_virtual_dj_path() -> Option<PathBuf> {
    default_path_for(&home()?, platform(), false)
}
pub fn detect_virtual_dj_installation(custom: Option<PathBuf>) -> Installation {
    let path = custom
        .filter(|value| !value.as_os_str().is_empty())
        .or_else(get_default_virtual_dj_path)
        .unwrap_or_default();
    let found = !path.as_os_str().is_empty() && path.exists();
    let text = path.to_string_lossy();
    let version = if found && (text.contains("Application Support") || text.contains("AppData")) {
        "v8"
    } else if found && text.contains("Documents") {
        "v7"
    } else {
        "unknown"
    };
    Installation {
        found,
        has_history: found && path.join("History").exists(),
        write_history_enabled: check_write_history(&path),
        path: path.clone(),
        version: version.into(),
    }
}
fn read_bounded(path: &Path) -> Result<String, Error> {
    let mut file = fs::File::open(path)?.take(MAX_HISTORY_BYTES + 1);
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    if content.len() as u64 > MAX_HISTORY_BYTES {
        return Err(Error("VirtualDJ file exceeds byte limit".into()));
    }
    Ok(content)
}
pub fn check_write_history(base: &Path) -> bool {
    if base.as_os_str().is_empty() {
        return false;
    }
    let Ok(content) = read_bounded(&base.join("settings.xml")) else {
        return true;
    };
    let expression = regex::RegexBuilder::new(r"<writeHistory>(.*?)</writeHistory>")
        .case_insensitive(true)
        .build()
        .unwrap();
    expression
        .captures(&content)
        .map(|captures| {
            matches!(
                captures[1].trim().to_lowercase().as_str(),
                "yes" | "true" | "1"
            )
        })
        .unwrap_or(true)
}
fn extract(tag: &str, line: &str) -> Option<String> {
    let start = line.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = line.rfind(&format!("</{tag}>"))?;
    if end < start {
        return None;
    }
    some(&line[start..end])
}
fn parse_int_prefix(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let mut end = 0;
    for (index, ch) in value.char_indices() {
        if ch.is_ascii_digit() || (index == 0 && (ch == '-' || ch == '+')) {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    value[..end].parse().ok()
}
fn clock_time(value: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    let mut fields = value.split(':');
    let hours = fields.next()?.trim().parse::<i64>().ok()?;
    let minutes = fields.next()?.trim().parse::<i64>().ok()?;
    let naive = now.date_naive().and_hms_opt(0, 0, 0)?;
    let date = naive
        .checked_add_signed(TimeDelta::try_hours(hours)?)?
        .checked_add_signed(TimeDelta::try_minutes(minutes)?)?;
    Local
        .from_local_datetime(&date)
        .earliest()
        .map(|date| date.with_timezone(&Utc))
}
/// Parse using an explicit local clock so fixture identities are reproducible.
pub fn parse_m3u(content: &str, now: DateTime<Local>) -> Result<Vec<Track>, Error> {
    if content.len() as u64 > MAX_HISTORY_BYTES {
        return Err(Error("VirtualDJ history exceeds byte limit".into()));
    }
    let lines: Vec<_> = content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut tracks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.starts_with("#EXTVDJ") {
            continue;
        }
        let mut track = Track {
            time: extract("time", line).and_then(|value| clock_time(&value, now)),
            last_play_time: extract("lastplaytime", line)
                .and_then(|value| parse_int_prefix(&value))
                .and_then(|value| DateTime::from_timestamp(value, 0)),
            artist: extract("artist", line),
            title: extract("title", line),
            remix: extract("remix", line),
            file_location: lines.get(index + 1).copied().unwrap_or("").into(),
            id: String::new(),
        };
        // JS JSON.stringify uses insertion order and exactly millisecond ISO dates.
        let mut fields = Vec::new();
        if let Some(time) = track.time {
            fields.push(format!(
                "\"time\":{}",
                serde_json::to_string(&time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap()
            ));
        }
        if let Some(time) = track.last_play_time {
            fields.push(format!(
                "\"lastPlayTime\":{}",
                serde_json::to_string(&time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap()
            ));
        }
        for (key, value) in [
            ("artist", &track.artist),
            ("title", &track.title),
            ("remix", &track.remix),
        ] {
            if let Some(value) = value {
                fields.push(format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    serde_json::to_string(value).unwrap()
                ));
            }
        }
        fields.push(format!(
            "\"fileLocation\":{}",
            serde_json::to_string(&track.file_location).unwrap()
        ));
        fields.push("\"id\":\"\"".into());
        track.id = format!("{:x}", md5::compute(format!("{{{}}}", fields.join(","))));
        tracks.push(track);
    }
    Ok(tracks)
}
pub struct VirtualDjM3uParser {
    pub history_file: PathBuf,
    pub base_path: PathBuf,
}
impl VirtualDjM3uParser {
    pub fn new(base_path: Option<PathBuf>) -> Self {
        Self {
            history_file: PathBuf::new(),
            base_path: base_path
                .filter(|value| !value.as_os_str().is_empty())
                .or_else(|| default_path_for(&home()?, platform(), true))
                .unwrap_or_default(),
        }
    }
    pub fn check_write_history(&self) -> bool {
        check_write_history(&self.base_path)
    }
    pub fn get_latest_track(&mut self) -> Result<Option<Track>, Error> {
        if self.base_path.as_os_str().is_empty() {
            return Ok(None);
        }
        let entries = match fs::read_dir(self.base_path.join("History")) {
            Ok(entries) => entries,
            Err(_) => return Ok(None),
        };
        let expression = regex::Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}\.m3u$").unwrap();
        let mut names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| expression.is_match(name))
            .collect::<Vec<_>>();
        names.sort();
        let Some(name) = names.pop() else {
            return Ok(None);
        };
        self.history_file = self.base_path.join("History").join(name);
        Ok(parse_m3u(&read_bounded(&self.history_file)?, Local::now())?.pop())
    }
}
pub fn history_payload(track: &Track) -> TrackPayload {
    let mut payload = base_payload(
        track.title.as_deref().unwrap_or(""),
        track.artist.as_deref().unwrap_or(""),
        &track.file_location,
    );
    payload.remix = track.remix.clone();
    payload
}
#[derive(Clone)]
pub struct HistoryOptions {
    pub base_path: Option<PathBuf>,
    pub poll_interval: Duration,
    pub logger: Option<Arc<dyn Logger>>,
}
impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            base_path: None,
            poll_interval: Duration::from_millis(5000),
            logger: None,
        }
    }
}
pub struct VirtualDjConnect {
    options: HistoryOptions,
    base_path: PathBuf,
    control: Arc<Control>,
    sender: SyncSender<Event>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}
impl VirtualDjConnect {
    pub fn new(options: HistoryOptions) -> Result<Self, Error> {
        let base_path = VirtualDjM3uParser::new(options.base_path.clone()).base_path;
        let control = Control::new(options.poll_interval)?;
        let (sender, events) = runtime::events();
        Ok(Self {
            options,
            base_path,
            control,
            sender,
            events,
            thread: None,
        })
    }
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }
    pub fn running(&self) -> bool {
        self.control.running.load(Ordering::Acquire)
    }
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }
    pub fn poll_interval(&self) -> Duration {
        self.control.interval()
    }
    pub fn set_poll_interval(&self, interval: Duration) -> Result<(), Error> {
        self.control.set_interval(interval)
    }
    pub fn check_write_history(&self) -> bool {
        check_write_history(&self.base_path)
    }
    pub fn last_error(&self) -> Option<Error> {
        self.control.failure.lock().unwrap().clone()
    }
    pub fn start(&mut self) -> Result<(), Error> {
        if self.running() {
            return Ok(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while self.events.try_recv().is_ok() {}
        if self.base_path.as_os_str().is_empty() {
            let message = "VirtualDJ installation not found".to_string();
            let _ = self.sender.try_send(Event::Error {
                message: message.clone(),
            });
            return Err(Error(message));
        }
        let mut parser = VirtualDjM3uParser::new(Some(self.base_path.clone()));
        let mut last_id = parser
            .get_latest_track()?
            .map(|track| track.id)
            .unwrap_or_default();
        self.control.start();
        self.control.emit(
            &self.sender,
            Event::Ready {
                base_path: self.base_path.to_string_lossy().into_owned(),
            },
        );
        let control = self.control.clone();
        let sender = self.sender.clone();
        let logger = self.options.logger.clone();
        self.thread = Some(thread::spawn(move || {
            while control.running.load(Ordering::Acquire) {
                control.wait();
                if !control.running.load(Ordering::Acquire) {
                    break;
                }
                match parser.get_latest_track() {
                    Ok(Some(track)) if track.id != last_id => {
                        last_id = track.id.clone();
                        control.emit(
                            &sender,
                            Event::Track {
                                track: Box::new(history_payload(&track)),
                            },
                        );
                    }
                    Err(error) => {
                        if let Some(logger) = &logger {
                            logger.log(crate::LogLevel::Error, &error.0);
                        }
                        control.emit(&sender, Event::Error { message: error.0 });
                    }
                    _ => {}
                }
            }
        }));
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        self.control.stop();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| Error("VirtualDJ history thread panicked".into()))?;
        }
        while self.events.try_recv().is_ok() {}
        Ok(())
    }
}
impl Drop for VirtualDjConnect {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
