use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use virtualdj_connect_rs::*;
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<String>>>,
    values: Arc<Mutex<HashMap<String, (u16, String)>>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::new(Mutex::new(HashMap::<String, (u16, String)>::new()));
        let stopped = stop.clone();
        let seen = requests.clone();
        let fields = values.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows can inherit the listener's nonblocking mode.
                        // A timeout does not switch it back: explicitly make
                        // accepted sockets blocking before reading HTTP headers.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut header = Vec::new();
                        let mut chunk = [0; 4096];
                        while header.len() < 16384 && !header.ends_with(b"\r\n\r\n") {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(count) => header.extend_from_slice(&chunk[..count]),
                            }
                        }
                        // A timeout/EOF or shutdown wakeup is not an HTTP request.
                        // Never reply and close while the client still has an
                        // incomplete request: unread data can cause a TCP reset.
                        if !header.ends_with(b"\r\n\r\n") {
                            continue;
                        }
                        let request = String::from_utf8_lossy(&header).into_owned();
                        let path = request
                            .lines()
                            .next()
                            .unwrap_or("")
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("/");
                        let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
                        let script = url
                            .query_pairs()
                            .find(|(key, _)| key == "script")
                            .map(|(_, value)| value.into_owned())
                            .unwrap_or_default();
                        seen.lock().unwrap().push(request);
                        let (status, body) = fields
                            .lock()
                            .unwrap()
                            .get(&script)
                            .cloned()
                            .unwrap_or((200, "no".into()));
                        let response=format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
            requests,
            values,
        }
    }
    fn set(&self, script: &str, value: &str) {
        self.values
            .lock()
            .unwrap()
            .insert(script.into(), (200, value.into()));
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
#[test]
fn real_http_sends_encoded_script_and_bearer_header_and_parameter() {
    let server = Server::new();
    server.set("deck 1 get_loaded_song 'title'", " 'Song' \n");
    let query = HttpQuery::new(&NetworkOptions {
        port: server.port,
        bearer: Some("secret-test-token".into()),
        ..NetworkOptions::default()
    })
    .unwrap();
    assert_eq!(
        query.query("deck 1 get_loaded_song 'title'").unwrap(),
        "'Song'"
    );
    let requests = server.requests.lock().unwrap();
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("authorization: bearer secret-test-token"));
    assert!(requests[0].contains("bearer=secret-test-token"));
    assert!(requests[0].contains("script=deck+1+get_loaded_song+%27title%27"));
}
#[test]
fn real_http_poll_delivers_full_track_and_live_transition() {
    let server = Server::new();
    for (script, value) in [
        ("get_clock", "1"),
        ("sandbox", "no"),
        ("deck 1 loaded", "yes"),
        ("deck 1 is_audible", "yes"),
        ("deck 1 get_loaded_song 'title'", "Title"),
        ("deck 1 get_loaded_song 'artist'", "Artist"),
        ("deck 1 get_loaded_song 'album'", "Album"),
        ("deck 1 get_loaded_song 'genre'", "House"),
        ("deck 1 get_loaded_song 'key'", "Am"),
        ("deck 1 get_bpm absolute", "128"),
        ("deck 1 get_time total", "4:03"),
        ("deck 1 get_filepath", "/Music/file.mp3"),
    ] {
        server.set(script, value);
    }
    let mut client = VirtualDjNetworkControl::new(NetworkOptions {
        port: server.port,
        decks: vec![1],
        poll_interval: Duration::from_millis(20),
        ..NetworkOptions::default()
    })
    .unwrap();
    client.start().unwrap();
    assert!(matches!(
        client
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        Event::Ready { .. }
    ));
    let event = client
        .events()
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(
        matches!(event,Event::Track{track}if track.title=="Title"&&track.bpm==Some(128.0)&&track.duration==Some(243.0)&&track.file_path.as_deref()==Some("/Music/file.mp3"))
    );
    assert_eq!(
        client
            .events()
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        Event::OnAir {
            active: true,
            deck: 1
        }
    );
    server.set("deck 1 is_audible", "no");
    assert_eq!(
        client
            .events()
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        Event::OnAir {
            active: false,
            deck: 1
        }
    );
    client.stop().unwrap();
}
#[test]
fn http_errors_and_oversized_response_are_bounded() {
    let server = Server::new();
    server
        .values
        .lock()
        .unwrap()
        .insert("get_clock".into(), (401, "Unauthorized".into()));
    let query = HttpQuery::new(&NetworkOptions {
        port: server.port,
        ..NetworkOptions::default()
    })
    .unwrap();
    assert!(query.query("get_clock").unwrap_err().0.contains("HTTP 401"));
    server.set("large-valid", &"x".repeat(128 * 1024));
    assert_eq!(query.query("large-valid").unwrap().len(), 128 * 1024);
    server.set("big", &"x".repeat(MAX_QUERY_BYTES as usize + 1));
    let error = query.query("big").unwrap_err();
    assert!(error.0.contains("byte limit"), "{}", error.0);
}
#[test]
fn unreachable_handshake_fails_within_configured_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut client = VirtualDjNetworkControl::new(NetworkOptions {
        port,
        request_timeout: Duration::from_millis(100),
        ..NetworkOptions::default()
    })
    .unwrap();
    let start = Instant::now();
    assert!(client.start().is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!client.running());
}

#[test]
fn fixture_waits_for_delayed_fragmented_request_before_responding() {
    let server = Server::new();
    server.set("get_clock", "fixture-clock");
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    // Exercise the accepted socket before there is anything to read. On
    // Windows a nonblocking accepted socket used to produce WouldBlock and
    // the fixture replied/closed before reqwest could send its request.
    thread::sleep(Duration::from_millis(30));
    assert!(server.requests.lock().unwrap().is_empty());
    stream
        .write_all(b"GET /query?script=get_clock HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(15));
    assert!(server.requests.lock().unwrap().is_empty());
    stream.write_all(b"\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 Fixture\r\n"));
    assert!(response.ends_with("fixture-clock"));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
