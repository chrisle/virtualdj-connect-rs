use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use virtualdj_connect_rs::*;
#[derive(Default)]
struct Stub {
    values: Mutex<HashMap<String, Result<String, Error>>>,
    calls: Mutex<Vec<String>>,
}
impl Query for Stub {
    fn query(&self, script: &str) -> Result<String, Error> {
        self.calls.lock().unwrap().push(script.into());
        self.values
            .lock()
            .unwrap()
            .get(script)
            .cloned()
            .unwrap_or(Err(Error("not supported".into())))
    }
}
impl Stub {
    fn set(&self, key: &str, value: &str) {
        self.values
            .lock()
            .unwrap()
            .insert(key.into(), Ok(value.into()));
    }
    fn fail(&self, key: &str) {
        self.values
            .lock()
            .unwrap()
            .insert(key.into(), Err(Error("HTTP 500".into())));
    }
    fn deck(&self, deck: u8, audible: bool, title: &str) {
        for (verb, value) in [
            ("loaded", "yes"),
            ("is_audible", if audible { "yes" } else { "no" }),
            ("get_loaded_song 'title'", title),
            ("get_loaded_song 'artist'", "Artist"),
            ("get_loaded_song 'album'", "Album"),
            ("get_loaded_song 'genre'", "House"),
            ("get_loaded_song 'key'", "Am"),
            ("get_bpm absolute", "128.50"),
            ("get_time total", "4:03"),
            ("get_filepath", "netsearch://bp123"),
        ] {
            self.set(&format!("deck {deck} {verb}"), value);
        }
    }
}
fn stub() -> Arc<Stub> {
    let stub = Arc::new(Stub::default());
    stub.set("get_clock", "12:00");
    stub.set("sandbox", "no");
    stub.deck(1, true, "Song");
    stub
}
fn snapshot(deck: u8, loaded: bool, audible: bool) -> DeckSnapshot {
    DeckSnapshot {
        deck,
        loaded,
        audible,
        title: format!("Song {deck}"),
        artist: "Artist".into(),
        path: format!("/{deck}.mp3"),
        ..DeckSnapshot::default()
    }
}
#[test]
fn deck_preference_and_loaded_fallback_preserve_array_order() {
    let decks = [
        snapshot(1, true, true),
        snapshot(2, true, true),
        snapshot(3, false, false),
    ];
    assert_eq!(pick_on_air_deck(&decks, None).unwrap().deck, 1);
    assert_eq!(pick_on_air_deck(&decks, Some(2)).unwrap().deck, 2);
    assert_eq!(pick_on_air_deck(&decks, Some(3)).unwrap().deck, 1);
    assert!(pick_on_air_deck(&[], None).is_none());
    let decks = [snapshot(4, true, false)];
    assert_eq!(pick_on_air_deck(&decks, None).unwrap().deck, 4);
}
#[test]
fn parse_helpers_match_vdjscript_booleans_quotes_errors_bpm_and_time() {
    for value in ["true", "YES", "on", "1"] {
        assert!(parse_bool(value));
    }
    for value in ["no", "false", "0", "'yes'", "error:-1"] {
        assert!(!parse_bool(value));
    }
    assert_eq!(strip_quotes(" 'A Song' "), "A Song");
    assert_eq!(strip_quotes("error:-2147467259"), "");
    assert_eq!(strip_quotes("'error:-1'"), "error:-1");
    assert_eq!(parse_bpm("'128.50'"), Some(128.5));
    assert_eq!(parse_bpm("0x80"), Some(128.0));
    assert_eq!(parse_bpm("Infinity"), None);
    assert_eq!(parse_duration("4:03"), Some(243.0));
    assert_eq!(parse_duration("01:04:03"), Some(3843.0));
    assert_eq!(parse_duration("243.5"), Some(243.5));
    assert_eq!(parse_duration("0:00"), Some(0.0));
    assert_eq!(parse_duration("0"), None);
    assert_eq!(parse_duration("bad"), None);
}
#[test]
fn network_metadata_and_optional_failures_match_source() {
    let query = stub();
    query.fail("deck 1 get_loaded_song 'album'");
    query.set("deck 1 get_filepath", "error:-2147467259");
    let mut state = NetworkState::default();
    let events = poll_network(query.as_ref(), &[1], &mut state);
    assert_eq!(events.len(), 2);
    let Event::Track { track } = &events[0] else {
        panic!("missing track")
    };
    assert_eq!(track.title, "Song");
    assert_eq!(track.album, None);
    assert_eq!(track.bpm, Some(128.5));
    assert_eq!(track.duration, Some(243.0));
    assert_eq!(track.file_location, "");
    assert!(track.file_path.is_none());
    assert_eq!(track.is_on_air, Some(true));
    assert_eq!(
        events[1],
        Event::OnAir {
            active: true,
            deck: 1
        }
    );
    assert!(poll_network(query.as_ref(), &[1], &mut state).is_empty());
}
#[test]
fn loaded_silent_track_then_audible_emits_only_live_transition() {
    let query = stub();
    query.set("deck 1 is_audible", "no");
    let mut state = NetworkState::default();
    let events = poll_network(query.as_ref(), &[1], &mut state);
    assert!(matches!(&events[0],Event::Track{track}if track.is_on_air==Some(false)));
    assert_eq!(
        events[1],
        Event::OnAir {
            active: false,
            deck: 1
        }
    );
    query.set("deck 1 is_audible", "yes");
    assert_eq!(
        poll_network(query.as_ref(), &[1], &mut state),
        vec![Event::OnAir {
            active: true,
            deck: 1
        }]
    );
    query.set("deck 1 loaded", "no");
    assert_eq!(
        poll_network(query.as_ref(), &[1], &mut state),
        vec![Event::OnAir {
            active: false,
            deck: 1
        }]
    );
    assert!(poll_network(query.as_ref(), &[1], &mut state).is_empty());
}
#[test]
fn sandbox_holds_signature_audibility_and_skips_deck_queries() {
    let query = stub();
    let mut state = NetworkState::default();
    poll_network(query.as_ref(), &[1], &mut state);
    query.set("sandbox", "'yes'");
    query.calls.lock().unwrap().clear();
    assert_eq!(
        poll_network(query.as_ref(), &[1], &mut state),
        vec![Event::Sandbox { active: true }]
    );
    assert!(state.on_air());
    assert_eq!(query.calls.lock().unwrap().as_slice(), ["sandbox"]);
    assert!(poll_network(query.as_ref(), &[1], &mut state).is_empty());
    query.set("sandbox", "no");
    assert_eq!(
        poll_network(query.as_ref(), &[1], &mut state),
        vec![Event::Sandbox { active: false }]
    );
}
#[test]
fn unsupported_sandbox_is_disabled_but_transport_failure_is_retried() {
    let query = stub();
    query.set("sandbox", "error:-2147467259");
    let mut state = NetworkState::default();
    poll_network(query.as_ref(), &[1], &mut state);
    assert!(!state.sandbox_query_supported);
    query.calls.lock().unwrap().clear();
    poll_network(query.as_ref(), &[1], &mut state);
    assert!(!query
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|script| script == "sandbox"));
    let mut state = NetworkState::default();
    query.fail("sandbox");
    poll_network(query.as_ref(), &[1], &mut state);
    assert!(state.sandbox_query_supported);
    query.set("sandbox", "yes");
    assert_eq!(
        poll_network(query.as_ref(), &[1], &mut state),
        vec![Event::Sandbox { active: true }]
    );
}
#[test]
fn crossfade_sticks_to_current_deck_and_hands_over_when_silent() {
    let mut state = NetworkState::default();
    state.update(&[snapshot(1, true, false), snapshot(2, true, true)]);
    assert_eq!(state.on_air_deck(), 2);
    assert!(state
        .update(&[snapshot(1, true, true), snapshot(2, true, true)])
        .is_empty());
    let events = state.update(&[snapshot(1, true, true), snapshot(2, true, false)]);
    assert!(matches!(&events[0],Event::Track{track}if track.deck==Some(1)));
    assert_eq!(state.on_air_deck(), 1);
}
#[test]
fn failed_required_field_omits_deck_and_beatport_metadata_is_preserved() {
    let query = stub();
    let deck = read_deck(query.as_ref(), 1).unwrap();
    let events = NetworkState::default().update(&[deck]);
    assert!(
        matches!(&events[0],Event::Track{track}if track.beatport_id==Some(123)&&track.file_path.is_none())
    );
    query.fail("deck 1 get_loaded_song 'title'");
    assert!(read_deck(query.as_ref(), 1).is_none());
}
#[test]
fn network_handshake_failure_is_retryable_and_stop_resets_state() {
    let query = stub();
    query.fail("get_clock");
    let mut client = VirtualDjNetworkControl::new(NetworkOptions {
        query: Some(query.clone()),
        decks: vec![1],
        poll_interval: Duration::from_secs(3600),
        ..NetworkOptions::default()
    })
    .unwrap();
    assert!(client.start().is_err());
    assert!(!client.running());
    assert!(matches!(
        client.events().recv().unwrap(),
        Event::Error { .. }
    ));
    query.set("get_clock", "1");
    client.start().unwrap();
    assert!(matches!(
        client
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        Event::Ready { .. }
    ));
    let mut tracks = 0;
    for _ in 0..2 {
        if matches!(
            client
                .events()
                .recv_timeout(Duration::from_secs(2))
                .unwrap(),
            Event::Track { .. }
        ) {
            tracks += 1;
        }
    }
    assert_eq!(tracks, 1);
    assert!(client.on_air());
    client.stop().unwrap();
    assert!(!client.running());
    assert!(!client.on_air());
    assert_eq!(client.on_air_deck(), 0);
}
