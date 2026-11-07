# virtualdj-connect-rs

A standalone Rust port of [Chris Le's virtualdj-connect](https://github.com/chrisle/virtualdj-connect), based on TypeScript version 1.2.4 at `3850cca1d690afa0dc19f2d3e13d89f94f1677d2`. Supports VirtualDJ's M3U history files and Network Control Plugin. Original MIT copyright and license are preserved in [LICENSE](LICENSE).

## History mode

```rust,no_run
use std::{path::PathBuf, time::Duration};
use virtualdj_connect_rs::{Event, HistoryOptions, VirtualDjConnect};
let mut connector = VirtualDjConnect::new(HistoryOptions {
    base_path: Some(PathBuf::from("/path/to/VirtualDJ")),
    poll_interval: Duration::from_secs(5),
    ..HistoryOptions::default()
})?;
connector.start()?;
while let Ok(event) = connector.events().recv_timeout(Duration::from_secs(10)) {
    if let Event::Track { track } = event {
        println!("{} - {}", track.artist, track.title);
    }
}
connector.stop()?;
# Ok::<(), virtualdj_connect_rs::Error>(())
```

`VirtualDjM3uParser` exposes `base_path`, `history_file`, `get_latest_track()`, and `check_write_history()`. `parse_m3u(content, local_clock)` parses synthetic fixtures reproducibly. It preserves the original ordered-JSON MD5 identity, millisecond ISO dates, Unix-second `lastplaytime`, CRLF handling, tag case sensitivity, and latest-date history-file selection. History monitoring seeds its cursor without replaying the current last entry; only later changed entries emit tracks. Stop/start reseeds the cursor.

Installation helpers are `get_default_virtual_dj_path`, `detect_virtual_dj_installation`, and `default_path_for(home, platform, require_database)`. As in the original, parser auto-detection requires `database.xml`, while installation detection accepts `settings.xml` too. VDJ8 locations are preferred over VDJ7. `writeHistory` is assumed enabled when the file/tag is unavailable, false when no base path is found, and true for `yes`, `true`, or `1` (case-insensitive).

## Network mode

```rust,no_run
use virtualdj_connect_rs::{NetworkOptions, VirtualDjNetworkControl};
let mut connector = VirtualDjNetworkControl::new(NetworkOptions {
    host: "127.0.0.1".into(),
    port: 8080,
    // bearer: Some("your configured plugin password".into()),
    ..NetworkOptions::default()
})?;
connector.start()?; // get_clock handshake before Ready
// Pull connector.events().try_recv() from your host's event loop.
connector.stop()?;
# Ok::<(), virtualdj_connect_rs::Error>(())
```

Defaults: host `127.0.0.1`, port `8080`, decks `[1,2,3,4]`, poll interval 1 second, per-request timeout 3 seconds. Passwords are sent both as the `bearer` query parameter and `Authorization: Bearer` header, matching the original. Error text excludes the password-bearing request URL. Use network access controls when querying another machine; the plugin protocol uses HTTP.

Public `query(script)` evaluates an additional VDJScript expression. `NetworkOptions.query` accepts `Arc<dyn Query>` for fixture injection, analogous to the original `fetchFn`. Custom implementations must honor bounded request duration if prompt stop is required. `HttpQuery` supplies bounded HTTP response reads and timeouts.

`running`, `base_url`, `poll_interval`, `set_poll_interval`, `sandboxed`, `on_air`, and `on_air_deck` expose current state. Network start performs a handshake and immediately polls after `Ready`. Stop clears signature, sandbox support, and audibility state.

Both modes expose a bounded event receiver through `events()`; use `try_recv` or `recv_timeout`. Events are `Ready { base_path }`, `Track { track: Box<TrackPayload> }`, `Error { message }`, `Sandbox { active }`, and `OnAir { active, deck }`. Pulling these events replaces Node EventEmitter subscriptions. Optional logging uses the `Logger` trait and `LogLevel`.

`TrackPayload` uses serde camelCase fields and omits absent optional values. File paths are omitted for `netsearch://` locations; Beatport locations expose `isBeatportStream` and `beatportId`. Missing title/artist become `Unknown Title`/`Unknown Artist`.

## Network behavior preserved

- Poll sandbox before reading decks. While sandbox is active, emit only sandbox transitions and hold the last song/audibility; skip all deck queries.
- Stop querying sandbox for the session after `error:-N` replies. Retry transport failures on later polls while continuing deck polling.
- Read loaded decks' audible state, title, artist, album, genre, key, BPM, duration, and filepath. Optional-field failures become empty values; required-field failures omit that deck.
- Prefer an audible deck. Keep the previously audible deck during a crossfade while it remains audible; otherwise use configured array order. Fall back to the first loaded deck when all are silent.
- Dedupe track changes on `artist|title|path`, independent of audibility. Emit `Track` before the separate `OnAir` transition. A silent loaded track becoming audible emits only `OnAir`, not a duplicate song.
- Going silent emits once. Sandbox holds the on-air state. Empty decks leave the last track standing.

Pure `NetworkState`, `DeckSnapshot`, `pick_on_air_deck`, `poll_network`, `read_deck`, and parsing helpers are exported for reproducible tests and embedders.

## Intentional differences and limits

This is a synchronous Rust API with dedicated background poll threads, not an async Node API. Polls do not overlap; parallel HTTP field reads within each deck preserve the original independent-field behavior. Stop cancels future reads and joins the active poll. With the built-in HTTP client, an in-flight deck read can take up to two request timeouts: one for `loaded`, then one for the parallel metadata phase (up to six seconds with the library defaults). A custom injected `Query` can block longer and is responsible for its own cancellation/timeout.

History files/settings are limited to 64 MiB, HTTP query bodies to 1 MiB, decks to 64 positive entries, and queued events to 256. A full queue fails closed, records `last_error()`, and stops polling. Stop clears queued events; restarting after a failed run joins it and drains stale events. Zero polling intervals are rejected. These are deliberate bounds, not behavior inferred from the original unbounded Node implementation.

The HTTP client does not follow redirects or environment proxies, preventing plugin bearer credentials from being forwarded unexpectedly. Network requests remain directed to the explicitly configured host. Injected transports can implement another policy when required.

Malformed UTF-8 history files are rejected, malformed/overflowing timestamps do not produce dates, and local times in a daylight-saving gap are omitted rather than normalized forward by JavaScript. Ordinary local times and Unix-second timestamps match the source. Very large Beatport IDs outside `u64` are omitted; ordinary IDs are preserved.

`VirtualDjDbTrack` in the original package is a type declaration only; it has no database reader implementation. This port does not invent one. No code accesses personal VirtualDJ folders in its fixture suite: custom temporary directories and loopback servers are used.

## Validation

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Tests exercise real temporary-history files and polling, installation preference and settings flags, startup seeding/restart, interval changes and prompt stop, deterministic network state transitions, optional/required failures, sandbox behavior, real loopback HTTP authentication/query encoding, full metadata and audibility transitions, bounded bodies, and unreachable handshakes.

For comparison with the pinned TypeScript implementation, supply an original checkout and an esbuild executable:

```sh
node scripts/differential.mjs /path/to/virtualdj-connect /path/to/esbuild
```

The differential driver compares five synthetic history fixtures (including exact MD5 identities/dates) and seven network poll transitions including crossfade preference and sandbox hold. It compiles the original source only for fixture execution, never starts VirtualDJ or reads personal settings.

CI is configured for macOS, Windows and Linux; configuration is not proof those jobs have run. Linux library tests do not add Linux support to the Now Playing desktop app. Physical VirtualDJ, packaged desktop, and cloud journeys remain separate verification gates. Public source: [chrisle/virtualdj-connect-rs](https://github.com/chrisle/virtualdj-connect-rs). The original-source differential tools passed the documented five history fixtures and seven network transitions locally; they do not establish real DJ-software or cloud parity.
