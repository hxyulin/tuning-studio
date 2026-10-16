//! Exercise the real HTTP/WebSocket host with mock memory and the bundled frontend.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::{connect, Message};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(address: &str, path: &str, origin: Option<&str>, body: &[u8]) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "{} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n",
        if body.is_empty() { "GET" } else { "POST" },
        body.len()
    )
    .unwrap();
    if let Some(origin) = origin {
        write!(stream, "Origin: {origin}\r\n").unwrap();
    }
    stream.write_all(b"\r\n").unwrap();
    stream.write_all(body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn the_browser_host_serves_assets_authenticates_and_releases_a_mock_session() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../studio-dwarf/tests/fixtures/test_arm.elf"
    );
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_studio-server"))
            .args([
                "--web",
                "--mock",
                "--no-open",
                "--elf",
                fixture,
                "--chip",
                "mock",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line.trim().strip_prefix("Tuning Studio: ").unwrap();
    let (base, token) = url.split_once('#').unwrap();
    let origin = base.split("/?").next().unwrap();
    let address = origin.strip_prefix("http://").unwrap();
    let response = http(address, "/", None, &[]);
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("<!doctype html>") || response.contains("<!DOCTYPE html>"));
    assert!(http(address, "/upload?token=wrong", Some(origin), b"elf").starts_with("HTTP/1.1 403"));
    assert!(http(
        address,
        &format!("/upload?token={token}"),
        Some("https://example.com"),
        b"elf"
    )
    .starts_with("HTTP/1.1 403"));
    let upload = http(
        address,
        &format!("/upload?token={token}"),
        Some(origin),
        b"elf",
    );
    assert!(upload.starts_with("HTTP/1.1 200"));
    let ws_url = format!("ws://{address}/ws?token={token}");
    assert!(connect(&*ws_url).is_err()); // no Origin
    for round in 0..2 {
        let mut request = ws_url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        let (mut socket, _) = connect(request).unwrap();
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
        }
        let mut call = |id: u32, method: &str, params: Value| {
            socket
                .send(Message::Text(
                    json!({"id":id, "method":method, "params":params})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            loop {
                let bytes = socket.read().unwrap().into_data();
                if bytes[4] != b'J' {
                    continue;
                }
                let value: Value = serde_json::from_slice(&bytes[5..]).unwrap();
                if value["id"] == id {
                    assert_eq!(value["ok"], true, "{value}");
                    return value["result"].clone();
                }
            }
        };
        let startup = call(1, "startup", json!({}));
        assert_eq!(startup["connect"]["chip"], "mock");
        let elf = call(2, "open_elf", json!({"path":fixture}));
        assert!(elf["roots"].is_array());
        let counter = elf["roots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["path"] == "global_counter")
            .unwrap()["ref"]
            .clone();
        call(
            5,
            "session_set_watches",
            json!({"watches":[{"id":1, "node":counter, "cell":null}]}),
        );
        call(
            3,
            "session_connect",
            json!({"session":1, "request":startup["connect"]}),
        );
        drop(call);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "no sample frame before deadline");
            let bytes = socket.read().unwrap().into_data();
            if bytes[4] == b'F' {
                assert_eq!(&bytes[13..17], b"TTS1");
                let n = u32::from_le_bytes(bytes[17..21].try_into().unwrap());
                assert!(n > 0);
                assert_eq!(u32::from_le_bytes(bytes[21..25].try_into().unwrap()), 1);
                break;
            }
        }
        // Closing with acquisition active must release the session for the next tab.
        socket.close(None).unwrap();
        drop(socket);
        if round == 0 {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}
