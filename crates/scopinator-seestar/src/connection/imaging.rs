use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, info, trace, warn};

use crate::command::ImagingCommand;
use crate::connection::reconnect::{self, ReconnectPolicy};
use crate::error::SeestarError;
use crate::protocol::frame::{FrameHeader, HEADER_SIZE, MAX_PAYLOAD_SIZE, PayloadKind};
use scopinator_types::FrameKind;

/// Connection timeout for TCP connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Heartbeat interval on the imaging port.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Request ids this client uses on the image socket. The device echoes the
/// low byte of the request id in every reply header, and pushed stream frames
/// repeat the id of the `begin_streaming` request, so these ids are how the
/// client tells its own replies apart. They are this client's choice, not
/// values defined by the device.
pub mod request_id {
    /// `begin_streaming` (and so every pushed stream frame).
    pub const STREAM: u8 = 21;
    /// The `test_connection` heartbeat.
    pub const HEARTBEAT: u8 = 99;
    /// First id of the block used for `get_stacked_img` requests. An image
    /// answering an id in this block is a stacked image.
    pub const STACK_FIRST: u8 = 0x80;
    /// First id of the block used for other one-shot image requests.
    pub const ONE_SHOT_FIRST: u8 = 0xC0;
    /// Number of ids in each block. Requests cycle through a block so that a
    /// late reply to an abandoned request is not taken for the next one's.
    pub const BLOCK_LEN: u8 = 0x40;

    /// True if `id` belongs to the `get_stacked_img` block.
    pub fn is_stack(id: u8) -> bool {
        (STACK_FIRST..ONE_SHOT_FIRST).contains(&id)
    }
}

/// A frame received from the imaging port: an image, or a text reply.
#[derive(Debug, Clone)]
pub struct ImageFrame {
    pub header: FrameHeader,
    pub data: Bytes,
    /// Coarse classification for consumers: `Status` for text replies,
    /// otherwise `Stack` or `Preview`.
    pub kind: FrameKind,
}

impl ImageFrame {
    /// What the payload is (text, error text, raw, zip, JPEG).
    pub fn payload_kind(&self) -> PayloadKind {
        self.header.payload_kind()
    }

    /// The payload of a text reply (`server connected!`, `done`, or an error
    /// message), if this is one.
    pub fn text(&self) -> Option<std::borrow::Cow<'_, str>> {
        matches!(
            self.payload_kind(),
            PayloadKind::Text | PayloadKind::ErrorText
        )
        .then(|| String::from_utf8_lossy(&self.data))
    }

    /// The image bytes ready for use: the payload itself for raw and JPEG
    /// frames, and the inflated first entry of the archive for zip frames.
    ///
    /// Fails with [`SeestarError::ImageRequest`] for an error-text frame and
    /// [`SeestarError::ImagePayload`] for anything else that is not an image.
    pub fn image_bytes(&self) -> Result<Bytes, SeestarError> {
        match self.payload_kind() {
            PayloadKind::Raw | PayloadKind::Jpeg => Ok(self.data.clone()),
            PayloadKind::Zip => inflate_first_entry(&self.data).map(Bytes::from),
            PayloadKind::ErrorText => Err(SeestarError::ImageRequest(
                String::from_utf8_lossy(&self.data).into_owned(),
            )),
            kind @ (PayloadKind::Text | PayloadKind::Unknown) => Err(SeestarError::ImagePayload(
                format!("frame is not an image ({kind:?})"),
            )),
        }
    }

    /// The error message, if this frame is the device refusing a request
    /// (e.g. `there is no image now`, `unknown mothod`).
    pub fn error_text(&self) -> Option<std::borrow::Cow<'_, str>> {
        (self.payload_kind() == PayloadKind::ErrorText).then(|| String::from_utf8_lossy(&self.data))
    }
}

/// Inflate the first entry of a zip archive (the device sends one, holding
/// the raw pixel data). The inflated size is capped at [`MAX_PAYLOAD_SIZE`].
fn inflate_first_entry(archive: &[u8]) -> Result<Vec<u8>, SeestarError> {
    use std::io::Read;
    let bad = |e: &dyn std::fmt::Display| SeestarError::ImagePayload(format!("zip: {e}"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| bad(&e))?;
    if zip.is_empty() {
        return Err(bad(&"archive has no entries"));
    }
    let entry = zip.by_index(0).map_err(|e| bad(&e))?;
    let limit = u64::from(MAX_PAYLOAD_SIZE);
    let mut out = Vec::new();
    entry
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| bad(&e))?;
    if out.len() as u64 > limit {
        return Err(bad(&"inflated entry exceeds the payload size limit"));
    }
    Ok(out)
}

/// Run the imaging connection loop (reconnecting on failure).
pub(crate) async fn run(
    addr: SocketAddr,
    frame_tx: broadcast::Sender<Arc<ImageFrame>>,
    connected: Arc<AtomicBool>,
    shutdown: watch::Receiver<bool>,
    mut cmd_rx: mpsc::Receiver<Vec<u8>>,
) {
    // Unit tests pin exact reconnect timing, so they run without jitter.
    let jitter = if cfg!(test) {
        Duration::ZERO
    } else {
        reconnect::DEFAULT_JITTER
    };
    let mut policy = ReconnectPolicy::with_jitter(jitter);

    loop {
        if *shutdown.borrow() {
            info!("imaging connection shutting down");
            break;
        }

        info!("connecting to telescope imaging at {addr}");

        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                if let Err(e) = stream.set_nodelay(true) {
                    warn!("failed to set TCP_NODELAY on imaging: {e}");
                }
                info!("connected to telescope imaging at {addr}");
                connected.store(true, Ordering::Release);
                policy.reset();

                run_imaging_connected(stream, &frame_tx, shutdown.clone(), &mut cmd_rx).await;

                connected.store(false, Ordering::Release);
                info!("imaging connection lost, will reconnect");
            }
            Ok(Err(e)) => {
                warn!("failed to connect imaging to {addr}: {e}");
            }
            Err(_) => {
                warn!("imaging connection to {addr} timed out");
            }
        }

        let wait = policy.next_backoff();
        tokio::time::sleep(wait).await;
    }
}

/// Run while connected.
///
/// Spawns a reader task (so frame reads are never mid-cancelled by write
/// events) and drives heartbeats + outbound commands inline on the write half.
async fn run_imaging_connected(
    stream: TcpStream,
    frame_tx: &broadcast::Sender<Arc<ImageFrame>>,
    mut shutdown: watch::Receiver<bool>,
    cmd_rx: &mut mpsc::Receiver<Vec<u8>>,
) {
    let (read_half, mut write_half) = tokio::io::split(stream);

    let reader_handle = tokio::spawn({
        let frame_tx = frame_tx.clone();
        let shutdown = shutdown.clone();
        async move { reader_loop(read_half, frame_tx, shutdown).await }
    });
    tokio::pin!(reader_handle);

    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    interval.tick().await; // skip first

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let mut msg = ImagingCommand::TestConnection.serialize(u64::from(request_id::HEARTBEAT));
                msg.extend_from_slice(b"\r\n");
                if let Err(e) = write_half.write_all(&msg).await {
                    debug!("imaging heartbeat write error: {e}");
                    break;
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                if let Err(e) = write_half.write_all(&cmd).await {
                    debug!("imaging cmd write error: {e}");
                    break;
                }
            }
            _ = shutdown.changed() => break,
            _ = &mut reader_handle => break,
        }
    }

    reader_handle.abort();
}

/// Reader loop: read frames until EOF, error, or shutdown.
async fn reader_loop(
    mut read_half: tokio::io::ReadHalf<TcpStream>,
    frame_tx: broadcast::Sender<Arc<ImageFrame>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut header_buf = [0u8; HEADER_SIZE];

    loop {
        tokio::select! {
            result = read_frame(&mut read_half, &mut header_buf) => {
                match result {
                    Ok(Some(frame)) => {
                        if let Some(text) = frame.error_text() {
                            warn!(
                                request_id = frame.header.request_id,
                                error = %text,
                                "imaging request refused"
                            );
                        }
                        trace!(
                            request_id = frame.header.request_id,
                            kind = ?frame.payload_kind(),
                            width = frame.header.width,
                            height = frame.header.height,
                            size = frame.header.size,
                            "received imaging frame"
                        );
                        let _ = frame_tx.send(Arc::new(frame));
                    }
                    Ok(None) => {
                        info!("imaging connection EOF");
                        break;
                    }
                    Err(e) => {
                        error!("imaging read error: {e}");
                        break;
                    }
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}

/// Read a single frame (header + payload) from the imaging stream.
async fn read_frame<R>(
    reader: &mut R,
    header_buf: &mut [u8; HEADER_SIZE],
) -> Result<Option<ImageFrame>, SeestarError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    // Read header
    match reader.read_exact(header_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SeestarError::Connection(e)),
    }

    let header = FrameHeader::parse(header_buf);

    // Without the magic the stream is out of step: nothing after this point
    // can be trusted, so the connection is dropped and re-established.
    if !header.has_valid_magic() {
        return Err(SeestarError::InvalidFrame);
    }

    if header.size > MAX_PAYLOAD_SIZE {
        return Err(SeestarError::FrameTooLarge {
            size: header.size,
            limit: MAX_PAYLOAD_SIZE,
        });
    }

    // Read payload
    let mut payload = vec![0u8; header.size as usize];
    match reader.read_exact(&mut payload).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SeestarError::Connection(e)),
    }

    // What the payload is comes from the header's dataType/imgType. Text
    // replies (heartbeat echoes, `done`, error messages) are status frames.
    // Among images, the request-id byte only says which of this client's
    // requests is being answered: the stacked-image request, or the stream.
    let kind = if !header.payload_kind().is_image() {
        FrameKind::Status
    } else if request_id::is_stack(header.request_id) {
        FrameKind::Stack
    } else {
        FrameKind::Preview
    };

    Ok(Some(ImageFrame {
        header,
        data: Bytes::from(payload),
        kind,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame::MAGIC;

    /// Request ids as the old tests named them.
    mod frame_id {
        pub const VIEW: u8 = super::request_id::STREAM;
        pub const STACK: u8 = super::request_id::STACK_FIRST;
    }

    /// A raw (dataType 2, 2 bytes/pixel) image header answering request `id`.
    fn make_header(size: u32, id: u8, width: u16, height: u16) -> [u8; HEADER_SIZE] {
        make_typed_header(size, id, width, height, 2)
    }

    fn make_typed_header(
        size: u32,
        id: u8,
        width: u16,
        height: u16,
        data_type: u8,
    ) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        buf[2..4].copy_from_slice(&2u16.to_be_bytes());
        buf[4..6].copy_from_slice(&80u16.to_be_bytes());
        buf[6..10].copy_from_slice(&size.to_be_bytes());
        buf[13] = 1;
        buf[14] = data_type;
        buf[15] = id;
        buf[16..18].copy_from_slice(&width.to_be_bytes());
        buf[18..20].copy_from_slice(&height.to_be_bytes());
        buf
    }

    #[tokio::test]
    async fn read_frame_success_preview() {
        let header = make_header(5, frame_id::VIEW, 100, 200);
        let payload = b"hello";

        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(payload)
            .build();

        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.header.size, 5);
        assert_eq!(frame.header.request_id, frame_id::VIEW);
        assert_eq!(frame.header.width, 100);
        assert_eq!(frame.header.height, 200);
        assert_eq!(&frame.data[..], b"hello");
        assert_eq!(frame.kind, FrameKind::Preview);
    }

    #[tokio::test]
    async fn read_frame_success_stack_maps_kind() {
        let header = make_header(3, frame_id::STACK, 4056, 3040);
        let payload = b"zip";

        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(payload)
            .build();

        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.kind, FrameKind::Stack);
    }

    #[tokio::test]
    async fn read_frame_non_image_id_maps_to_status() {
        // A `test_connection` heartbeat echo is a text reply (dataType 0):
        // Status, whatever request id it carries.
        let header = make_typed_header(17, 99, 0, 0, 0);
        let payload = [0u8; 17];
        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(&payload)
            .build();
        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.kind, FrameKind::Status);
        assert!(!frame.kind.is_image());
    }

    #[tokio::test]
    async fn kind_comes_from_the_payload_type_not_the_request_id() {
        async fn kind_of(header: [u8; HEADER_SIZE]) -> FrameKind {
            let mut mock = tokio_test::io::Builder::new()
                .read(&header)
                .read(b"abc")
                .build();
            let mut buf = [0u8; HEADER_SIZE];
            read_frame(&mut mock, &mut buf).await.unwrap().unwrap().kind
        }
        // A text reply to `begin_streaming` ("done") or to the stacked-image
        // request carries those requests' ids and is still not an image.
        for data_type in [0, 1] {
            for id in [request_id::STREAM, request_id::STACK_FIRST, 20, 2] {
                let header = make_typed_header(3, id, 1080, 1920, data_type);
                assert_eq!(kind_of(header).await, FrameKind::Status);
            }
        }
        // An image answering any other request id is a preview, not dropped.
        for id in [0, 20, 77, request_id::ONE_SHOT_FIRST, 255] {
            let header = make_typed_header(3, id, 1080, 1920, 2);
            assert_eq!(kind_of(header).await, FrameKind::Preview);
        }
        for id in [request_id::STACK_FIRST, request_id::ONE_SHOT_FIRST - 1] {
            let header = make_typed_header(3, id, 1080, 1920, 3);
            assert_eq!(kind_of(header).await, FrameKind::Stack);
        }
    }

    #[tokio::test]
    async fn error_text_reply_is_exposed_as_an_error() {
        let text = b"there is no image now";
        let header = make_typed_header(text.len() as u32, request_id::STACK_FIRST, 0, 0, 1);
        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(text)
            .build();
        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.kind, FrameKind::Status);
        assert_eq!(frame.error_text().as_deref(), Some("there is no image now"));
        assert_eq!(frame.text().as_deref(), Some("there is no image now"));
        let err = frame.image_bytes().unwrap_err();
        assert!(
            matches!(&err, SeestarError::ImageRequest(m) if m == "there is no image now"),
            "got {err:?}"
        );
    }

    fn frame_with(data_type: u8, payload: Vec<u8>) -> ImageFrame {
        let raw = make_typed_header(
            payload.len() as u32,
            request_id::STACK_FIRST,
            2,
            2,
            data_type,
        );
        ImageFrame {
            header: FrameHeader::parse(&raw),
            data: Bytes::from(payload),
            kind: FrameKind::Stack,
        }
    }

    #[test]
    fn image_bytes_inflates_a_zip_payload() {
        use std::io::Write;
        let pixels: Vec<u8> = (0..=255).cycle().take(4096).collect();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("raw_data", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&pixels).unwrap();
        let archive = writer.finish().unwrap().into_inner();
        assert_ne!(archive, pixels);

        assert_eq!(
            &frame_with(3, archive).image_bytes().unwrap()[..],
            &pixels[..]
        );
        // Raw payloads pass through untouched.
        assert_eq!(
            &frame_with(2, pixels.clone()).image_bytes().unwrap()[..],
            &pixels[..]
        );
    }

    #[test]
    fn image_bytes_rejects_a_damaged_zip_and_text() {
        let err = frame_with(3, b"not a zip archive".to_vec())
            .image_bytes()
            .unwrap_err();
        assert!(matches!(err, SeestarError::ImagePayload(_)), "got {err:?}");
        let err = frame_with(0, b"done".to_vec()).image_bytes().unwrap_err();
        assert!(matches!(err, SeestarError::ImagePayload(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn bad_magic_is_a_broken_connection() {
        let mut header = make_header(5, request_id::STREAM, 100, 200);
        header[0] = b'{';
        header[1] = b'"';
        // The payload is never read: the stream cannot be trusted.
        let mut mock = tokio_test::io::Builder::new().read(&header).build();
        let mut buf = [0u8; HEADER_SIZE];
        let err = read_frame(&mut mock, &mut buf).await.unwrap_err();
        assert!(matches!(err, SeestarError::InvalidFrame), "got {err:?}");
    }

    #[tokio::test]
    async fn read_frame_zero_payload_succeeds() {
        let header = make_typed_header(0, 2, 0, 0, 0);
        let mut mock = tokio_test::io::Builder::new().read(&header).build();
        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.header.size, 0);
        assert!(frame.data.is_empty());
    }

    #[tokio::test]
    async fn read_frame_eof_before_header_returns_none() {
        let mut empty = tokio::io::empty();
        let mut buf = [0u8; HEADER_SIZE];
        let result = read_frame(&mut empty, &mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_frame_partial_header_returns_none() {
        // Only 10 bytes of an 80-byte header — read_exact errors with UnexpectedEof
        // which is mapped to Ok(None).
        let mut mock = tokio_test::io::Builder::new().read(&[0u8; 10]).build();
        let mut buf = [0u8; HEADER_SIZE];
        let result = read_frame(&mut mock, &mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_frame_truncated_payload_returns_none() {
        // Header says 10 bytes but only 3 are available before EOF.
        let header = make_header(10, frame_id::VIEW, 0, 0);
        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(&[0u8; 3])
            .build();
        let mut buf = [0u8; HEADER_SIZE];
        let result = read_frame(&mut mock, &mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_frame_oversized_payload_errors() {
        // Rejection boundary (Forge task: "Add imaging allocation pressure
        // tests"): one byte over the cap and the extreme u32::MAX claim are
        // both rejected *before* the payload buffer is allocated, so a
        // hostile header can never make `read_frame` reserve more than
        // MAX_PAYLOAD_SIZE.
        for claimed in [MAX_PAYLOAD_SIZE + 1, u32::MAX] {
            let header = make_header(claimed, frame_id::VIEW, 0, 0);
            let mut mock = tokio_test::io::Builder::new().read(&header).build();
            let mut buf = [0u8; HEADER_SIZE];
            let err = read_frame(&mut mock, &mut buf).await.unwrap_err();
            match err {
                SeestarError::FrameTooLarge { size, limit } => {
                    assert_eq!(size, claimed);
                    assert_eq!(limit, MAX_PAYLOAD_SIZE);
                }
                other => panic!("expected FrameTooLarge, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn read_frame_exactly_at_cap_is_accepted() {
        // Characterization (Forge task: "Add imaging allocation pressure
        // tests"): the cap check is `size > MAX_PAYLOAD_SIZE`, so a header
        // announcing exactly the cap (50 MiB) is accepted, and the full
        // 50 MiB buffer is reserved before a single payload byte has
        // arrived. A true end-to-end success at the cap would mean pushing
        // 50 MiB through the stream, which these tests deliberately avoid;
        // acceptance is pinned via the truncated-stream path instead: no
        // FrameTooLarge error, the read proceeds into the payload phase,
        // then EOF maps to Ok(None).
        let header = make_header(MAX_PAYLOAD_SIZE, frame_id::VIEW, 1920, 1080);
        let mut mock = tokio_test::io::Builder::new().read(&header).build();
        let mut buf = [0u8; HEADER_SIZE];

        let frame = read_frame(&mut mock, &mut buf)
            .await
            .expect("exactly MAX_PAYLOAD_SIZE must pass the size check");
        assert!(frame.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn read_frame_near_cap_header_truncated_returns_none() {
        // Characterization (Forge task: "Add imaging allocation pressure
        // tests"): the imaging port has no auth gate, and a header just under
        // the cap (49 MiB) is accepted by the size check; `read_frame` then
        // eagerly reserves the full 49 MiB payload buffer before reading a
        // single payload byte. When the peer truncates the connection
        // mid-payload, the partial `read_exact` surfaces as Ok(None) —
        // exactly the "clean EOF" a graceful close would produce — so the
        // caller cannot distinguish a truncated frame from an orderly
        // shutdown, and the large reservation was made for nothing. A caller
        // would want a distinct error here (and ideally a read strategy that
        // does not pre-reserve the announced size). Today's behavior:
        // Ok(None), promptly, no hang.
        const NEAR_CAP: u32 = 49 * 1024 * 1024;

        let header = make_header(NEAR_CAP, frame_id::VIEW, 1920, 1080);
        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(&[0u8; 1024]) // a sliver of payload, then the stream is cut
            .build();
        let mut buf = [0u8; HEADER_SIZE];

        // Paused-clock guard: fires only (deterministically) if `read_frame`
        // were to wedge on a truncated near-cap frame instead of erroring.
        let frame = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut mock, &mut buf))
            .await
            .expect("read_frame must not hang on a truncated near-cap frame")
            .expect("49 MiB is under the cap: must not be rejected as FrameTooLarge");

        assert!(frame.is_none());
    }

    #[tokio::test]
    async fn read_frame_sequential_large_frames_complete() {
        // Characterization (Forge task: "Add imaging allocation pressure
        // tests"): each `read_frame` call allocates one payload buffer and
        // hands it to the caller inside `ImageFrame`; `read_frame` itself
        // accumulates nothing between calls. RSS cannot be measured from a
        // unit test, so pin the bounded shape instead: several sequential
        // large-header frames each complete, and dropping each frame releases
        // its allocation before the next one is read. (1 MiB per frame keeps
        // the total bytes written far below the "no tens of megabytes of
        // payload" rule while still exercising the large-allocation path.)
        const FRAME_BYTES: usize = 1024 * 1024;

        let header = make_header(FRAME_BYTES as u32, frame_id::VIEW, 1920, 1080);
        let payload = vec![0x5A; FRAME_BYTES];

        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(&payload)
            .read(&header)
            .read(&payload)
            .read(&header)
            .read(&payload)
            .read(&header)
            .read(&payload)
            .build();
        let mut buf = [0u8; HEADER_SIZE];

        for i in 0..4 {
            let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
            assert_eq!(frame.header.size as usize, FRAME_BYTES, "frame {i}");
            assert_eq!(frame.data.len(), FRAME_BYTES, "frame {i}");
            assert!(frame.data.iter().all(|&b| b == 0x5A), "frame {i}");
            // Drop the frame (and its 1 MiB payload) before reading the next.
            drop(frame);
        }

        // All frames consumed; the stream ends in a clean EOF.
        assert!(read_frame(&mut mock, &mut buf).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_frame_at_max_payload_size_succeeds() {
        // Boundary: exactly MAX_PAYLOAD_SIZE is allowed (use a small surrogate
        // to avoid allocating 50 MB in tests).
        let header = make_header(16, frame_id::VIEW, 0, 0);
        let payload = vec![0xAB; 16];
        let mut mock = tokio_test::io::Builder::new()
            .read(&header)
            .read(&payload)
            .build();
        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(frame.data.len(), 16);
        assert!(frame.data.iter().all(|&b| b == 0xAB));
    }

    #[tokio::test]
    async fn read_frame_split_reads_reassemble_correctly() {
        // Sender may split header and payload across multiple read() calls;
        // read_frame must use read_exact, which loops until full buffer is read.
        let header = make_header(8, frame_id::VIEW, 0, 0);
        let payload = b"abcdefgh";
        let mut mock = tokio_test::io::Builder::new()
            .read(&header[..40])
            .read(&header[40..])
            .read(&payload[..3])
            .read(&payload[3..])
            .build();
        let mut buf = [0u8; HEADER_SIZE];
        let frame = read_frame(&mut mock, &mut buf).await.unwrap().unwrap();
        assert_eq!(&frame.data[..], b"abcdefgh");
    }
}
