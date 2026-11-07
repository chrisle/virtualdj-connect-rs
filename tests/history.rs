mod common;
use chrono::{Local, TimeZone};
use common::Temp;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use virtualdj_connect_rs::*;
fn entry(artist: &str, title: &str, path: &str) -> String {
    format!("#EXTVDJ:<artist>{artist}</artist><title>{title}</title>\n{path}\n")
}
fn fixture() -> (Temp, std::path::PathBuf) {
    let temp = Temp::new();
    fs::create_dir(temp.0.join("History")).unwrap();
    let path = temp.0.join("History/2026-11-07.m3u");
    fs::write(&path, entry("Previous", "Session", "/old.mp3")).unwrap();
    (temp, path)
}
fn next_track(connector: &VirtualDjConnect) -> TrackPayload {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match connector
            .events()
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Event::Track { track } => return *track,
            Event::Error { message } => panic!("{message}"),
            _ => {}
        }
    }
}
#[test]
fn parses_metadata_unix_seconds_crlf_and_exact_legacy_md5() {
    let now = Local.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap();
    let input="#EXTM3U\r\n#EXTVDJ:<time>14:30</time><lastplaytime>1704196200</lastplaytime><artist>Årtist</artist><title>Title</title><remix>Mix</remix>\r\n/path.mp3\r\n";
    let track = parse_m3u(input, now).unwrap().pop().unwrap();
    assert_eq!(track.artist.as_deref(), Some("Årtist"));
    assert_eq!(track.file_location, "/path.mp3");
    assert_eq!(track.last_play_time.unwrap().timestamp(), 1704196200);
    assert!(track.time.is_some());
    assert_eq!(track.id.len(), 32);
    // No dates: JSON property order and omitted fields must match JS JSON.stringify.
    let track = parse_m3u(&entry("A", "T", "/x"), now)
        .unwrap()
        .pop()
        .unwrap();
    let expected = r#"{"artist":"A","title":"T","fileLocation":"/x","id":""}"#;
    assert_eq!(track.id, format!("{:x}", md5::compute(expected)));
}
#[test]
fn parser_selects_latest_date_and_handles_empty_or_missing_history() {
    let temp = Temp::new();
    let mut parser = VirtualDjM3uParser::new(Some(temp.0.clone()));
    assert!(parser.get_latest_track().unwrap().is_none());
    fs::create_dir(temp.0.join("History")).unwrap();
    fs::write(
        temp.0.join("History/2024-01-01.m3u"),
        entry("A", "Old", "/old"),
    )
    .unwrap();
    fs::write(
        temp.0.join("History/2025-02-03.m3u"),
        entry("A", "New", "/new"),
    )
    .unwrap();
    fs::write(
        temp.0.join("History/not-history.m3u"),
        entry("A", "Ignore", "/ignore"),
    )
    .unwrap();
    assert_eq!(
        parser.get_latest_track().unwrap().unwrap().title.as_deref(),
        Some("New")
    );
    assert!(parser.history_file.ends_with("2025-02-03.m3u"));
    fs::write(&parser.history_file, "").unwrap();
    assert!(parser.get_latest_track().unwrap().is_none());
}
#[test]
fn settings_flags_and_installation_preference_match_source() {
    let temp = Temp::new();
    let v8 = temp.0.join("Library/Application Support/VirtualDJ");
    let v7 = temp.0.join("Documents/VirtualDJ");
    fs::create_dir_all(&v8).unwrap();
    fs::create_dir_all(&v7).unwrap();
    fs::write(v7.join("database.xml"), "").unwrap();
    fs::write(
        v8.join("settings.xml"),
        "<writeHistory> YES </writeHistory>",
    )
    .unwrap();
    assert_eq!(default_path_for(&temp.0, "darwin", false), Some(v8.clone()));
    assert_eq!(default_path_for(&temp.0, "darwin", true), Some(v7));
    assert_eq!(
        detect_virtual_dj_installation(Some(v8.clone())).version,
        "v8"
    );
    for (value, expected) in [
        ("yes", true),
        ("true", true),
        ("1", true),
        ("no", false),
        ("false", false),
        ("0", false),
    ] {
        fs::write(
            v8.join("settings.xml"),
            format!("<WRITEHISTORY>{value}</WRITEHISTORY>"),
        )
        .unwrap();
        assert_eq!(check_write_history(&v8), expected);
    }
    fs::remove_file(v8.join("settings.xml")).unwrap();
    assert!(check_write_history(&v8));
    assert!(!check_write_history(Path::new("")));
    assert_eq!(default_path_for(&temp.0, "linux", false), None);
}
#[test]
fn payload_stream_detection_defaults_and_missing_location_match_source() {
    let now = Local::now();
    let track = parse_m3u("#EXTVDJ:<remix>Mix</remix>\nnetsearch://bp12345\n", now)
        .unwrap()
        .pop()
        .unwrap();
    let payload = history_payload(&track);
    assert_eq!(payload.title, "Unknown Title");
    assert_eq!(payload.artist, "Unknown Artist");
    assert_eq!(payload.beatport_id, Some(12345));
    assert!(payload.file_path.is_none());
    assert_eq!(payload.remix.as_deref(), Some("Mix"));
    let track = parse_m3u("#EXTVDJ:<title>Last without path</title>", now)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(track.file_location, "");
}
#[test]
fn real_history_poll_seeds_cursor_detects_append_and_reseeds_on_restart() {
    let (temp, path) = fixture();
    let mut connector = VirtualDjConnect::new(HistoryOptions {
        base_path: Some(temp.0.clone()),
        poll_interval: Duration::from_millis(20),
        ..HistoryOptions::default()
    })
    .unwrap();
    connector.start().unwrap();
    assert!(matches!(
        connector
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        Event::Ready { .. }
    ));
    assert!(connector
        .events()
        .recv_timeout(Duration::from_millis(50))
        .is_err());
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(entry("New", "Track", "netsearch://bp42").as_bytes())
        .unwrap();
    file.flush().unwrap();
    let payload = next_track(&connector);
    assert_eq!(payload.title, "Track");
    assert_eq!(payload.beatport_id, Some(42));
    assert!(connector
        .events()
        .recv_timeout(Duration::from_millis(50))
        .is_err());
    connector.stop().unwrap();
    connector.start().unwrap();
    assert!(matches!(
        connector
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        Event::Ready { .. }
    ));
    assert!(connector
        .events()
        .recv_timeout(Duration::from_millis(50))
        .is_err());
    connector.stop().unwrap();
}
#[test]
fn interval_change_and_stop_interrupt_long_wait() {
    let (temp, path) = fixture();
    let mut connector = VirtualDjConnect::new(HistoryOptions {
        base_path: Some(temp.0.clone()),
        poll_interval: Duration::from_secs(3600),
        ..HistoryOptions::default()
    })
    .unwrap();
    connector.start().unwrap();
    connector
        .events()
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    connector
        .set_poll_interval(Duration::from_millis(10))
        .unwrap();
    assert_eq!(connector.poll_interval(), Duration::from_millis(10));
    fs::write(path, entry("A", "Changed", "/new")).unwrap();
    assert_eq!(next_track(&connector).title, "Changed");
    connector
        .set_poll_interval(Duration::from_secs(3600))
        .unwrap();
    let before = Instant::now();
    connector.stop().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(connector.set_poll_interval(Duration::ZERO).is_err());
}
