use crate::{
    base_payload,
    runtime::{self, Control},
    some, Error, Event, Logger, TrackPayload,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub const MAX_QUERY_BYTES: u64 = 1024 * 1024;
pub trait Query: Send + Sync {
    fn query(&self, script: &str) -> Result<String, Error>;
}
#[derive(Clone)]
pub struct NetworkOptions {
    pub host: String,
    pub port: u16,
    pub bearer: Option<String>,
    pub decks: Vec<u8>,
    pub poll_interval: Duration,
    pub request_timeout: Duration,
    pub logger: Option<Arc<dyn Logger>>,
    pub query: Option<Arc<dyn Query>>,
}
impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            bearer: None,
            decks: vec![1, 2, 3, 4],
            poll_interval: Duration::from_millis(1000),
            request_timeout: Duration::from_secs(3),
            logger: None,
            query: None,
        }
    }
}
pub struct HttpQuery {
    client: reqwest::blocking::Client,
    base_url: reqwest::Url,
    bearer: Option<String>,
}
impl HttpQuery {
    pub fn new(options: &NetworkOptions) -> Result<Self, Error> {
        if options.request_timeout.is_zero() {
            return Err(Error("request timeout must be positive".into()));
        }
        let host = if options.host.contains(':') && !options.host.starts_with('[') {
            format!("[{}]", options.host)
        } else {
            options.host.clone()
        };
        let base_url = reqwest::Url::parse(&format!("http://{host}:{}", options.port))
            .map_err(|error| Error(error.to_string()))?;
        if base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.path() != "/"
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(Error("invalid Network Control host".into()));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| Error(error.to_string()))?;
        Ok(Self {
            client,
            base_url,
            bearer: options.bearer.clone().filter(|bearer| !bearer.is_empty()),
        })
    }
    pub fn base_url(&self) -> String {
        self.base_url.as_str().trim_end_matches('/').into()
    }
}
impl Query for HttpQuery {
    fn query(&self, script: &str) -> Result<String, Error> {
        let mut url = self
            .base_url
            .join("/query")
            .map_err(|error| Error(error.to_string()))?;
        url.query_pairs_mut().append_pair("script", script);
        if let Some(bearer) = &self.bearer {
            url.query_pairs_mut().append_pair("bearer", bearer);
        }
        let mut request = self.client.get(url);
        if let Some(bearer) = &self.bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().map_err(|_| {
            Error(format!(
                "Network Control request failed for script {script:?}"
            ))
        })?;
        if !response.status().is_success() {
            return Err(Error(format!(
                "HTTP {} on query {script:?}",
                response.status().as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_QUERY_BYTES)
        {
            return Err(Error("Network Control response exceeds byte limit".into()));
        }
        let mut bytes = Vec::new();
        response.take(MAX_QUERY_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_QUERY_BYTES {
            return Err(Error("Network Control response exceeds byte limit".into()));
        }
        Ok(String::from_utf8_lossy(&bytes).trim().into())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckSnapshot {
    pub deck: u8,
    pub loaded: bool,
    pub audible: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub key: String,
    pub bpm: Option<f64>,
    pub duration: Option<f64>,
    pub path: String,
}
pub fn pick_on_air_deck(
    snapshots: &[DeckSnapshot],
    prefer_deck: Option<u8>,
) -> Option<&DeckSnapshot> {
    snapshots
        .iter()
        .find(|deck| deck.loaded && deck.audible && Some(deck.deck) == prefer_deck)
        .or_else(|| snapshots.iter().find(|deck| deck.loaded && deck.audible))
        .or_else(|| snapshots.iter().find(|deck| deck.loaded))
}
pub fn is_vdj_error(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    let Some(value) = lower.strip_prefix("error:") else {
        return false;
    };
    let value = value.strip_prefix('-').unwrap_or(value);
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}
pub fn strip_quotes(raw: &str) -> String {
    let raw = raw.trim();
    if is_vdj_error(raw) {
        return String::new();
    }
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        raw[1..raw.len() - 1].into()
    } else {
        raw.into()
    }
}
pub fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}
fn number(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if let Some(digits) = raw.to_ascii_lowercase().strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix)
                .ok()
                .map(|number| number as f64);
        }
    }
    raw.parse::<f64>().ok().filter(|number| number.is_finite())
}
pub fn parse_bpm(raw: &str) -> Option<f64> {
    let raw = strip_quotes(raw);
    if raw.is_empty() {
        return None;
    }
    number(&raw).filter(|number| *number > 0.0)
}
pub fn parse_duration(raw: &str) -> Option<f64> {
    let raw = strip_quotes(raw);
    if raw.is_empty() {
        return None;
    }
    let numeric = regex::Regex::new(r"^\d+(\.\d+)?$").unwrap();
    if numeric.is_match(&raw) {
        return number(&raw).filter(|value| *value > 0.0);
    }
    let parts = raw.split(':').map(number).collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [minutes, seconds] => Some(minutes * 60.0 + seconds),
        [hours, minutes, seconds] => Some(hours * 3600.0 + minutes * 60.0 + seconds),
        _ => None,
    }
}
fn network_payload(deck: &DeckSnapshot) -> TrackPayload {
    let mut payload = base_payload(&deck.title, &deck.artist, &deck.path);
    payload.album = some(&deck.album);
    payload.genre = some(&deck.genre);
    payload.key = some(&deck.key);
    payload.bpm = deck.bpm;
    payload.duration = deck.duration;
    payload.deck = Some(deck.deck);
    payload.is_on_air = Some(deck.audible);
    payload
}
/// Deterministic state machine exposed for fixture/differential testing. Sandbox
/// holds both the last signature and live on-air state until released.
#[derive(Debug, Clone)]
pub struct NetworkState {
    last_signature: String,
    pub sandbox_active: bool,
    pub sandbox_query_supported: bool,
    pub last_on_air: Option<(bool, u8)>,
}
impl Default for NetworkState {
    fn default() -> Self {
        Self {
            last_signature: String::new(),
            sandbox_active: false,
            sandbox_query_supported: true,
            last_on_air: None,
        }
    }
}
impl NetworkState {
    pub fn on_air(&self) -> bool {
        self.last_on_air.is_some_and(|state| state.0)
    }
    pub fn on_air_deck(&self) -> u8 {
        self.last_on_air
            .filter(|state| state.0)
            .map(|state| state.1)
            .unwrap_or(0)
    }
    pub fn sandbox(&mut self, active: bool) -> Vec<Event> {
        if self.sandbox_active == active {
            return Vec::new();
        }
        self.sandbox_active = active;
        vec![Event::Sandbox { active }]
    }
    pub fn update(&mut self, snapshots: &[DeckSnapshot]) -> Vec<Event> {
        if self.sandbox_active {
            return Vec::new();
        }
        let mut events = Vec::new();
        let picked = pick_on_air_deck(snapshots, Some(self.on_air_deck()));
        let (active, deck) = if let Some(picked) = picked {
            let signature = format!("{}|{}|{}", picked.artist, picked.title, picked.path);
            if signature != self.last_signature {
                self.last_signature = signature;
                events.push(Event::Track {
                    track: Box::new(network_payload(picked)),
                });
            }
            (picked.audible, picked.deck)
        } else {
            (false, self.last_on_air.map(|state| state.1).unwrap_or(0))
        };
        let changed = if active {
            self.last_on_air != Some((true, deck))
        } else {
            !self.last_on_air.is_some_and(|state| !state.0)
        };
        if changed {
            self.last_on_air = Some((active, deck));
            events.push(Event::OnAir { active, deck });
        }
        events
    }
}
/// Read one deck. Required-field failures omit that deck; optional-field failures
/// become empty fields. Parallel fields match the original Promise.all behavior.
pub fn read_deck(query: &dyn Query, deck: u8) -> Option<DeckSnapshot> {
    let loaded = parse_bool(&query.query(&format!("deck {deck} loaded")).ok()?);
    if !loaded {
        return Some(DeckSnapshot {
            deck,
            ..DeckSnapshot::default()
        });
    }
    let verbs = [
        "is_audible",
        "get_loaded_song 'title'",
        "get_loaded_song 'artist'",
        "get_loaded_song 'album'",
        "get_loaded_song 'genre'",
        "get_loaded_song 'key'",
        "get_bpm absolute",
        "get_time total",
        "get_filepath",
    ];
    let results = thread::scope(|scope| {
        let handles = verbs
            .into_iter()
            .map(|verb| scope.spawn(move || query.query(&format!("deck {deck} {verb}"))))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(Error("query callback panicked".into())))
            })
            .collect::<Vec<_>>()
    });
    if results[..3].iter().any(Result::is_err) {
        return None;
    }
    let values = results
        .into_iter()
        .map(Result::unwrap_or_default)
        .collect::<Vec<_>>();
    Some(DeckSnapshot {
        deck,
        loaded: true,
        audible: parse_bool(&values[0]),
        title: strip_quotes(&values[1]),
        artist: strip_quotes(&values[2]),
        album: strip_quotes(&values[3]),
        genre: strip_quotes(&values[4]),
        key: strip_quotes(&values[5]),
        bpm: parse_bpm(&values[6]),
        duration: parse_duration(&values[7]),
        path: strip_quotes(&values[8]),
    })
}
pub fn poll_network(query: &dyn Query, decks: &[u8], state: &mut NetworkState) -> Vec<Event> {
    poll_cancellable(query, decks, state, None)
}
fn poll_cancellable(
    query: &dyn Query,
    decks: &[u8],
    state: &mut NetworkState,
    control: Option<&Control>,
) -> Vec<Event> {
    let active = if state.sandbox_query_supported {
        match query.query("sandbox") {
            Ok(raw) if is_vdj_error(&raw) => {
                state.sandbox_query_supported = false;
                false
            }
            Ok(raw) => parse_bool(&strip_quotes(&raw)),
            Err(_) => false,
        }
    } else {
        false
    };
    let mut events = state.sandbox(active);
    if active {
        return events;
    }
    let mut snapshots = Vec::new();
    for deck in decks {
        if control.is_some_and(|control| !control.running.load(Ordering::Acquire)) {
            return Vec::new();
        }
        if let Some(snapshot) = read_deck(query, *deck) {
            snapshots.push(snapshot);
        }
    }
    events.extend(state.update(&snapshots));
    events
}
pub struct VirtualDjNetworkControl {
    options: NetworkOptions,
    base_url: String,
    query: Arc<dyn Query>,
    control: Arc<Control>,
    state: Arc<Mutex<NetworkState>>,
    sender: SyncSender<Event>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}
impl VirtualDjNetworkControl {
    pub fn new(options: NetworkOptions) -> Result<Self, Error> {
        if options.decks.len() > 64 || options.decks.contains(&0) {
            return Err(Error(
                "decks must be positive and at most 64 entries".into(),
            ));
        }
        let http = HttpQuery::new(&options)?;
        let base_url = http.base_url();
        let query = options.query.clone().unwrap_or_else(|| Arc::new(http));
        let control = Control::new(options.poll_interval)?;
        let (sender, events) = runtime::events();
        Ok(Self {
            options,
            base_url,
            query,
            control,
            state: Arc::new(Mutex::new(NetworkState::default())),
            sender,
            events,
            thread: None,
        })
    }
    pub fn running(&self) -> bool {
        self.control.running.load(Ordering::Acquire)
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
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
    pub fn sandboxed(&self) -> bool {
        self.state.lock().unwrap().sandbox_active
    }
    pub fn on_air(&self) -> bool {
        self.state.lock().unwrap().on_air()
    }
    pub fn on_air_deck(&self) -> u8 {
        self.state.lock().unwrap().on_air_deck()
    }
    pub fn last_error(&self) -> Option<Error> {
        self.control.failure.lock().unwrap().clone()
    }
    pub fn query(&self, script: &str) -> Result<String, Error> {
        self.query.query(script)
    }
    pub fn start(&mut self) -> Result<(), Error> {
        if self.running() {
            return Ok(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while self.events.try_recv().is_ok() {}
        if let Err(error) = self.query("get_clock") {
            let message = format!("Network Control handshake failed: {error}");
            let _ = self.sender.try_send(Event::Error {
                message: message.clone(),
            });
            return Err(Error(message));
        }
        *self.state.lock().unwrap() = NetworkState::default();
        self.control.start();
        self.control.emit(
            &self.sender,
            Event::Ready {
                base_path: self.base_url.clone(),
            },
        );
        let query = self.query.clone();
        let decks = self.options.decks.clone();
        let control = self.control.clone();
        let state = self.state.clone();
        let sender = self.sender.clone();
        let logger = self.options.logger.clone();
        self.thread = Some(thread::spawn(move || {
            let mut local = NetworkState::default();
            while control.running.load(Ordering::Acquire) {
                let events = poll_cancellable(query.as_ref(), &decks, &mut local, Some(&control));
                if !control.running.load(Ordering::Acquire) {
                    break;
                }
                *state.lock().unwrap() = local.clone();
                for event in events {
                    if !control.emit(&sender, event) {
                        break;
                    }
                }
                if let Some(logger) = &logger {
                    logger.log(crate::LogLevel::Trace, "Network Control poll finished");
                }
                control.wait();
            }
        }));
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        self.control.stop();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| Error("VirtualDJ network thread panicked".into()))?;
        }
        *self.state.lock().unwrap() = NetworkState::default();
        while self.events.try_recv().is_ok() {}
        Ok(())
    }
}
impl Drop for VirtualDjNetworkControl {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
