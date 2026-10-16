//! Loopback-only browser host. A per-launch capability and exact Origin check protect
//! local files and hardware from requests made by unrelated websites.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};
use studio_carriers::probe::{ProbeConfig, ProbeLink};

use super::{open_mock, parse, Out, Server, StudioApp, OUTBOX};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend");

#[derive(Default)]
struct Options {
    elf: Option<PathBuf>,
    chip: Option<String>,
    probe: Option<String>,
    speed_khz: Option<u32>,
    port: u16,
    reset: bool,
    flash: bool,
    no_open: bool,
    mock: bool,
    read_only: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut result = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--web" => {}
                "--elf" => result.elf = Some(args.next().ok_or("--elf requires a path")?.into()),
                "--chip" => result.chip = Some(args.next().ok_or("--chip requires a chip")?),
                "--probe" => result.probe = Some(args.next().ok_or("--probe requires a selector")?),
                "--speed-khz" => {
                    result.speed_khz = Some(
                        args.next()
                            .ok_or("--speed-khz requires a value")?
                            .parse()
                            .map_err(|_| "invalid SWD speed")?,
                    );
                }
                "--port" => {
                    result.port = args
                        .next()
                        .ok_or("--port requires a value")?
                        .parse()
                        .map_err(|_| "invalid port")?;
                }
                "--reset" => result.reset = true,
                "--flash" => {
                    result.flash = true;
                    result.reset = true;
                }
                "--no-open" => result.no_open = true,
                "--mock" => result.mock = true,
                "--read-only" => result.read_only = true,
                _ => return Err(format!("unknown web option: {arg}")),
            }
        }
        if result.reset && (result.mock || result.read_only) {
            return Err("reset/flash cannot be used with --mock or --read-only".into());
        }
        if result.reset && (result.elf.is_none() || result.chip.is_none()) {
            return Err("reset/flash requires --elf and --chip".into());
        }
        if let Some(path) = &result.elf {
            result.elf = Some(path.canonicalize().map_err(|e| format!("ELF: {e}"))?);
        }
        Ok(result)
    }

    fn startup(&self) -> Value {
        let connect = self
            .chip
            .as_ref()
            .filter(|_| self.elf.is_some())
            .map(|chip| {
                json!({
                    "carrier": "probe", "probe": self.probe, "chip": chip,
                    "speedKhz": self.speed_khz, "port": null, "rateHz": 100.0,
                })
            });
        json!({ "elfPath": self.elf, "connect": connect,
            "connectDefaults": { "chip": self.chip, "probe": self.probe,
                "speedKhz": self.speed_khz }, "watches": [] })
    }
}

struct WebState {
    options: Options,
    origin: String,
    token: String,
    occupied: AtomicBool,
    uploads: tempfile::TempDir,
}

#[derive(Deserialize)]
struct Access {
    token: String,
}

impl WebState {
    fn authorized(&self, access: &Access, headers: &HeaderMap) -> bool {
        access.token == self.token
            && headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(&self.origin)
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))?;
    if !options.mock {
        studio_carriers::probe::init_hid_on_main_thread();
    }
    if options.reset {
        eprintln!(
            "{} {}",
            if options.flash {
                "Flashing and resetting"
            } else {
                "Resetting"
            },
            options.elf.as_ref().expect("validated ELF").display()
        );
        ProbeLink::prepare(
            &ProbeConfig {
                selector: options.probe.clone(),
                chip: options.chip.clone().expect("validated chip"),
                speed_khz: options.speed_khz,
            },
            options.elf.as_ref().expect("validated ELF"),
            options.flash,
        )?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, options.port))
                    .await?;
            let address: SocketAddr = listener.local_addr()?;
            let state = Arc::new(WebState {
                options,
                origin: format!("http://{address}"),
                token: uuid::Uuid::new_v4().to_string(),
                occupied: AtomicBool::new(false),
                uploads: tempfile::tempdir()?,
            });
            let url = format!("{}/?host=web#{}", state.origin, state.token);
            println!("Tuning Studio: {url}");
            if !state.options.no_open {
                if let Err(error) = open::that(&url) {
                    eprintln!("Could not open the browser: {error}. Open the printed URL.");
                }
            }
            let router = Router::new()
                .route("/ws", get(upgrade))
                .route("/upload", post(upload))
                .fallback(get(asset))
                .layer(DefaultBodyLimit::max(64 << 20))
                .with_state(state);
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
}

async fn asset(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = ASSETS.get_file(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = if path.ends_with(".js") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html"
    } else {
        "application/octet-stream"
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        file.contents(),
    )
        .into_response()
}

async fn upload(
    State(state): State<Arc<WebState>>,
    Query(access): Query<Access>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if !state.authorized(&access, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = state.uploads.path().join(uuid::Uuid::new_v4().to_string());
    match tokio::fs::write(&path, bytes).await {
        Ok(()) => Json(json!({ "path": path })).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn upgrade(
    State(state): State<Arc<WebState>>,
    Query(access): Query<Access>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.authorized(&access, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(1 << 20)
        .on_upgrade(move |socket| connection(socket, state))
}

async fn connection(socket: WebSocket, state: Arc<WebState>) {
    let (mut writer, mut reader) = socket.split();
    if state.occupied.swap(true, Ordering::AcqRel) {
        let _ = writer.send(Message::Close(None)).await;
        return;
    }
    let (tx, rx) = mpsc::sync_channel(OUTBOX);
    let out = Out(tx);
    let app = if state.options.mock {
        StudioApp::with_probe_opener(Arc::new(open_mock))
    } else {
        StudioApp::new()
    };
    let app = if state.options.read_only {
        let open = app.probe_opener();
        StudioApp::with_probe_opener(Arc::new(move |config, elf| {
            open(config, elf)
                .map(|link| Box::new(super::ReadOnlyLink(link)) as Box<dyn studio_carriers::Link>)
        }))
        .without_rtt()
    } else {
        app
    };
    let app = Arc::new(app);
    let events = out.clone();
    app.set_event_sink(Arc::new(move |event| {
        events.json(&json!({ "type": "app_event", "event": event }));
    }));
    let server = Arc::new(Server {
        startup: state.options.startup(),
        app: app.clone(),
        out: out.clone(),
        mock: state.options.mock,
        read_only: state.options.read_only,
    });
    let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let forward = tokio::task::spawn_blocking(move || {
        while !async_tx.is_closed() {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(bytes) => {
                    if async_tx.blocking_send(bytes).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    let send = tokio::spawn(async move {
        while let Some(bytes) = async_rx.recv().await {
            if tokio::time::timeout(
                Duration::from_secs(5),
                writer.send(Message::Binary(bytes.into())),
            )
            .await
            .map_or(true, |r| r.is_err())
            {
                break;
            }
        }
    });
    loop {
        let message = tokio::select! {
            message = reader.next() => message,
            _ = tokio::signal::ctrl_c() => break,
        };
        let Some(Ok(Message::Text(body))) = message else {
            break;
        };
        let server = server.clone();
        if tokio::task::spawn_blocking(move || match parse(body.as_bytes()) {
            Ok((id, call)) => {
                let result = call.and_then(|call| server.run(call));
                server.out.respond(id, result);
            }
            Err(error) => server.out.respond(Value::Null, Err(error)),
        })
        .await
        .is_err()
        {
            break;
        }
    }
    send.abort();
    let _ = send.await;
    let _ = forward.await;
    let _ = tokio::task::spawn_blocking(move || {
        let _ = app.can_request(studio_app::can::Request::Disconnect);
        app.disconnect();
        app.stop_stream();
    })
    .await;
    state.occupied.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaching_is_the_default_and_flash_requires_an_explicit_target() {
        let options = Options::parse(["--web".into()].into_iter()).unwrap();
        assert!(!options.flash && !options.reset);
        assert!(Options::parse(["--web", "--flash"].map(String::from).into_iter()).is_err());
        assert!(
            Options::parse(["--web", "--reset", "--mock"].map(String::from).into_iter()).is_err()
        );
    }

    #[test]
    fn only_the_launch_token_and_same_origin_authorize_hardware_requests() {
        let state = WebState {
            options: Options::default(),
            origin: "http://127.0.0.1:1234".into(),
            token: "secret".into(),
            occupied: AtomicBool::new(false),
            uploads: tempfile::tempdir().unwrap(),
        };
        let access = Access {
            token: "secret".into(),
        };
        let mut headers = HeaderMap::new();
        assert!(!state.authorized(&access, &headers));
        headers.insert(header::ORIGIN, "https://example.com".parse().unwrap());
        assert!(!state.authorized(&access, &headers));
        headers.insert(header::ORIGIN, state.origin.parse().unwrap());
        assert!(state.authorized(&access, &headers));
        assert!(!state.authorized(
            &Access {
                token: "wrong".into()
            },
            &headers
        ));
    }
}
