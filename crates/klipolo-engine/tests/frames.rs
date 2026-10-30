use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use klipolo_engine::frames::server::FrameServer;
use klipolo_engine::frames::trailer::{
    FLAG_PLAYBACK, FORMAT_NV12, FrameMeta, TRAILER_LEN, TrailerError, append_trailer, read_trailer,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const ORIGIN: &str = "tauri://localhost";

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn meta(time_us: i64) -> FrameMeta {
    FrameMeta {
        width: 2,
        height: 1,
        stride: 8,
        format: 0,
        flags: FLAG_PLAYBACK,
        seq: 7,
        session: 3,
        revision: 42,
        time_us,
        sent_ms: 1.5,
        due_ms: 2.5,
    }
}

fn frame(time_us: i64) -> Bytes {
    let mut buf = vec![255u8; 8];
    append_trailer(&mut buf, &meta(time_us));
    Bytes::from(buf)
}

async fn connect(port: u16, token: &str, origin: Option<&str>) -> Result<Client, String> {
    let mut req = format!("ws://127.0.0.1:{port}/frames?t={token}")
        .into_client_request()
        .unwrap();
    if let Some(origin) = origin {
        req.headers_mut().insert("Origin", origin.parse().unwrap());
    }
    connect_async(req)
        .await
        .map(|(ws, _)| ws)
        .map_err(|e| e.to_string())
}

async fn next_time(ws: &mut Client) -> i64 {
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("a frame")
        .unwrap()
        .unwrap();
    read_trailer(&msg.into_data()).unwrap().time_us
}

fn is_closed(msg: &Option<Result<Message, tokio_tungstenite::tungstenite::Error>>) -> bool {
    matches!(msg, None | Some(Ok(Message::Close(_))) | Some(Err(_)))
}

#[test]
fn trailer_round_trips_and_rejects_corruption() {
    let buf = frame(1_000_000);
    assert_eq!(buf.len(), 8 + TRAILER_LEN);
    assert_eq!(read_trailer(&buf), Ok(meta(1_000_000)));
    let mut bad = buf.to_vec();
    let at = bad.len() - TRAILER_LEN;
    bad[at] = b'X';
    assert_eq!(read_trailer(&bad), Err(TrailerError::BadMagic));
    let mut narrow = vec![0u8; 8];
    append_trailer(
        &mut narrow,
        &FrameMeta {
            stride: 4,
            ..meta(0)
        },
    );
    assert_eq!(read_trailer(&narrow), Err(TrailerError::BadSize));
    let mut unknown = buf.to_vec();
    unknown[at + 16] = 9; // the format field: RGBA8 (0) and NV12 (1) only
    assert_eq!(read_trailer(&unknown), Err(TrailerError::BadFormat));
    assert_eq!(read_trailer(&[0u8; 10]), Err(TrailerError::TooShort));
    // NV12: a Y plane of stride × height, then the UV plane (stride × height / 2).
    let mut nv12 = vec![0u8; 8 * 2 + 8];
    let nv12_meta = FrameMeta {
        width: 8,
        height: 2,
        stride: 8,
        format: FORMAT_NV12,
        ..meta(0)
    };
    append_trailer(&mut nv12, &nv12_meta);
    assert_eq!(read_trailer(&nv12), Ok(nv12_meta));
    nv12.drain(..1);
    assert_eq!(
        read_trailer(&nv12),
        Err(TrailerError::BadSize),
        "a short UV plane"
    );
}

#[tokio::test]
async fn wrong_token_or_origin_is_refused() {
    let (server, _sink) = FrameServer::start(vec![ORIGIN.into()]).await.unwrap();
    assert!(
        connect(server.port, "nope", Some(ORIGIN))
            .await
            .unwrap_err()
            .contains("403")
    );
    assert!(
        connect(server.port, &server.token, Some("https://evil.example"))
            .await
            .unwrap_err()
            .contains("403")
    );
    assert!(
        connect(server.port, &server.token, None)
            .await
            .unwrap_err()
            .contains("403")
    );
}

#[tokio::test]
async fn newest_frame_wins() {
    let (server, sink) = FrameServer::start(vec![ORIGIN.into()]).await.unwrap();
    for t in 1..=3 {
        sink.publish(frame(t));
    }
    let mut ws = connect(server.port, &server.token, Some(ORIGIN))
        .await
        .unwrap();
    assert_eq!(
        next_time(&mut ws).await,
        3,
        "a new client gets only the newest frame"
    );
    sink.publish(frame(4));
    assert_eq!(next_time(&mut ws).await, 4);
}

#[tokio::test]
async fn a_second_client_replaces_the_first() {
    let (server, sink) = FrameServer::start(vec![ORIGIN.into()]).await.unwrap();
    let mut first = connect(server.port, &server.token, Some(ORIGIN))
        .await
        .unwrap();
    let mut second = connect(server.port, &server.token, Some(ORIGIN))
        .await
        .unwrap();
    sink.publish(frame(9));
    assert_eq!(next_time(&mut second).await, 9);
    let closed = tokio::time::timeout(Duration::from_secs(2), first.next())
        .await
        .unwrap();
    assert!(is_closed(&closed), "{closed:?}");
}

#[tokio::test]
async fn stopping_the_server_closes_the_client() {
    let (server, _sink) = FrameServer::start(vec![ORIGIN.into()]).await.unwrap();
    let mut ws = connect(server.port, &server.token, Some(ORIGIN))
        .await
        .unwrap();
    drop(server);
    let closed = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap();
    assert!(is_closed(&closed), "{closed:?}");
}
