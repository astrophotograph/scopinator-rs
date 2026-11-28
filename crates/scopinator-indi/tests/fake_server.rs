//! The fake INDI server itself, driven with a plain socket.

mod common;

use std::time::Duration;

use common::FakeIndi;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const CONNECTION: &str = r#"<defSwitchVector device="Scope" name="CONNECTION" state="Idle" perm="rw" rule="OneOfMany"><defSwitch name="CONNECT">Off</defSwitch><defSwitch name="DISCONNECT">On</defSwitch></defSwitchVector>"#;
const COORDINATES: &str = r#"<defNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Idle" perm="rw"><defNumber name="RA" format="%g" min="0" max="24" step="0">1</defNumber></defNumberVector>"#;

/// Read from `socket` until `want` bytes have come, or two seconds pass.
async fn read_some(socket: &mut TcpStream, want: usize) -> String {
    let mut got = Vec::new();
    let mut chunk = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while got.len() < want {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await;
    String::from_utf8_lossy(&got).into_owned()
}

async fn eventually(check: impl Fn() -> bool) {
    for _ in 0..100 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("it did not happen within two seconds");
}

#[tokio::test]
async fn asking_for_properties_gets_the_definitions_in_order() {
    let fake = FakeIndi::start().await;
    fake.define(CONNECTION).define(COORDINATES);
    let mut socket = TcpStream::connect(fake.addr).await.unwrap();
    socket
        .write_all(b"<getProperties version=\"1.7\"/>\n")
        .await
        .unwrap();
    let got = read_some(&mut socket, CONNECTION.len() + COORDINATES.len()).await;
    assert_eq!(got, format!("{CONNECTION}{COORDINATES}"));
    assert_eq!(fake.received(), ["<getProperties version=\"1.7\"/>"]);
    assert_eq!(fake.count("getProperties"), 1);
}

#[tokio::test]
async fn what_is_sent_reaches_every_client() {
    let fake = FakeIndi::start().await;
    let mut first = TcpStream::connect(fake.addr).await.unwrap();
    let mut second = TcpStream::connect(fake.addr).await.unwrap();
    eventually(|| fake.connection_count() == 2).await;
    let message = r#"<message device="Scope" message="hello"/>"#;
    fake.send(message);
    assert_eq!(read_some(&mut first, message.len()).await, message);
    // In pieces, it is the same bytes.
    fake.send_in_chunks(message, 7);
    assert_eq!(
        read_some(&mut second, message.len() * 2).await,
        message.repeat(2)
    );
}

#[tokio::test]
async fn a_new_vector_is_recorded_and_answered_if_a_reply_was_scripted() {
    let fake = FakeIndi::start().await;
    let reply = r#"<setSwitchVector device="Scope" name="CONNECTION" state="Ok"><oneSwitch name="CONNECT">On</oneSwitch></setSwitchVector>"#;
    fake.on_new("Scope", "CONNECTION", reply);
    let mut socket = TcpStream::connect(fake.addr).await.unwrap();

    let connect = "<newSwitchVector device=\"Scope\" name=\"CONNECTION\">\n  <oneSwitch name=\"CONNECT\">On</oneSwitch>\n</newSwitchVector>";
    // Split across two writes: it is one element all the same.
    socket.write_all(&connect.as_bytes()[..30]).await.unwrap();
    socket.flush().await.unwrap();
    socket.write_all(&connect.as_bytes()[30..]).await.unwrap();
    assert_eq!(read_some(&mut socket, reply.len()).await, reply);
    let recorded = fake.received_new("Scope", "CONNECTION");
    assert_eq!(recorded, [connect]);
    assert_eq!(
        common::member(&recorded[0], "oneSwitch", "CONNECT").as_deref(),
        Some("On")
    );

    // Another property: recorded, not answered.
    let park = r#"<newSwitchVector device="Scope" name="TELESCOPE_PARK"><oneSwitch name="PARK">On</oneSwitch></newSwitchVector>"#;
    socket.write_all(park.as_bytes()).await.unwrap();
    eventually(|| fake.received_new("Scope", "TELESCOPE_PARK").len() == 1).await;
    assert_eq!(read_some(&mut socket, 1).await, "");
    assert!(fake.received_new("Other", "CONNECTION").is_empty());
}

#[tokio::test]
async fn dropped_connections_close_and_new_ones_are_accepted() {
    let fake = FakeIndi::start().await;
    let mut socket = TcpStream::connect(fake.addr).await.unwrap();
    eventually(|| fake.connection_count() == 1).await;
    fake.drop_connections();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte)).await;
    assert!(matches!(closed, Ok(Ok(0))), "{closed:?}");

    let mut again = TcpStream::connect(fake.addr).await.unwrap();
    again
        .write_all(b"<getProperties version=\"1.7\"/>")
        .await
        .unwrap();
    eventually(|| fake.connection_count() == 2 && fake.count("getProperties") == 1).await;
}
