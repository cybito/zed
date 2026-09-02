use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{LazyLock, Mutex},
    time::Duration,
};

const PROTOCOL_VERSION: u8 = 1;
const MAXIMUM_FRAME_SIZE: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_millis(150);

static BRIDGE: LazyLock<Mutex<SquirrelBridge>> =
    LazyLock::new(|| Mutex::new(SquirrelBridge::default()));

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrepareResult {
    Ready,
    CompositionActive,
    Unavailable,
    Faulted(String),
}

pub(crate) fn set_mode(enabled: bool, owner: u64, command_mode: bool) -> PrepareResult {
    with_bridge(|bridge| {
        bridge.configure(enabled);
        if !enabled {
            PrepareResult::Ready
        } else if command_mode {
            bridge.acquire_for_owner(owner)
        } else {
            bridge.release_for_owner(owner)
        }
    })
}

fn with_bridge<T>(callback: impl FnOnce(&mut SquirrelBridge) -> T) -> T {
    let mut bridge = BRIDGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    callback(&mut bridge)
}

struct SquirrelBridge {
    enabled: bool,
    stream: Option<UnixStream>,
    socket_path: PathBuf,
    next_request_id: u64,
    lease: Option<Lease>,
    lease_owner: Option<u64>,
}

impl Default for SquirrelBridge {
    fn default() -> Self {
        Self {
            enabled: false,
            stream: None,
            socket_path: socket_path(),
            next_request_id: 0,
            lease: None,
            lease_owner: None,
        }
    }
}

#[derive(Clone)]
struct Lease {
    session_token: String,
    session_generation: u64,
    lease_id: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum BridgeMethod {
    Status,
    AcquireCommand,
    ReleaseToInsert,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    version: u8,
    request_id: String,
    method: BridgeMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_id: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    version: u8,
    request_id: String,
    status: String,
    message: Option<String>,
    session_token: Option<String>,
    session_generation: Option<u64>,
    lease_id: Option<String>,
    #[allow(dead_code)]
    ascii_mode: Option<bool>,
    #[allow(dead_code)]
    composition_active: Option<bool>,
}

enum BridgeStatus<'a> {
    Ok,
    NoActiveSession,
    SessionUnavailable,
    StaleSession,
    CompositionActive,
    NoLease,
    UserOverride,
    Unknown(&'a str),
}

impl Response {
    fn status(&self) -> BridgeStatus<'_> {
        match self.status.as_str() {
            "ok" => BridgeStatus::Ok,
            "no_active_session" => BridgeStatus::NoActiveSession,
            "session_unavailable" => BridgeStatus::SessionUnavailable,
            "stale_session" => BridgeStatus::StaleSession,
            "composition_active" => BridgeStatus::CompositionActive,
            "no_lease" => BridgeStatus::NoLease,
            "user_override" => BridgeStatus::UserOverride,
            status => BridgeStatus::Unknown(status),
        }
    }
}

impl SquirrelBridge {
    fn configure(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.disconnect();
        self.enabled = enabled;
    }

    fn acquire_for_owner(&mut self, owner: u64) -> PrepareResult {
        let result = self.acquire_command();
        if result == PrepareResult::Ready {
            self.lease_owner = Some(owner);
        }
        result
    }

    fn release_for_owner(&mut self, owner: u64) -> PrepareResult {
        if self.lease_owner == Some(owner) {
            self.release_to_insert()
        } else {
            PrepareResult::Ready
        }
    }

    fn acquire_command(&mut self) -> PrepareResult {
        self.acquire_command_with_retry(true)
    }

    fn acquire_command_with_retry(&mut self, retry_stale_session: bool) -> PrepareResult {
        let status = match self.request(BridgeMethod::Status, None) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        match status.status() {
            BridgeStatus::Ok => {}
            BridgeStatus::NoActiveSession | BridgeStatus::SessionUnavailable => {
                self.clear_lease();
                return PrepareResult::Unavailable;
            }
            BridgeStatus::StaleSession
            | BridgeStatus::CompositionActive
            | BridgeStatus::NoLease
            | BridgeStatus::UserOverride => return self.fail(response_error(&status)),
            BridgeStatus::Unknown(response_status) => {
                return self.fail(response_error_for_status(
                    response_status,
                    status.message.as_deref(),
                ));
            }
        }
        if self.lease.as_ref().is_some_and(|lease| {
            status.session_token.as_deref() == Some(lease.session_token.as_str())
                && status.session_generation == Some(lease.session_generation)
                && status.lease_id.as_deref() == Some(lease.lease_id.as_str())
        }) {
            return PrepareResult::Ready;
        }
        self.clear_lease();

        let Some(session_token) = status.session_token else {
            return self.fail("Squirrel omitted the active session token".to_owned());
        };
        let Some(session_generation) = status.session_generation else {
            return self.fail("Squirrel omitted the active session generation".to_owned());
        };
        let candidate = Lease {
            session_token,
            session_generation,
            lease_id: String::new(),
        };
        let response = match self.request(BridgeMethod::AcquireCommand, Some(&candidate)) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        match response.status() {
            BridgeStatus::NoActiveSession | BridgeStatus::SessionUnavailable => {
                self.clear_lease();
                return PrepareResult::Unavailable;
            }
            BridgeStatus::StaleSession => {
                return if retry_stale_session {
                    self.acquire_command_with_retry(false)
                } else {
                    PrepareResult::Unavailable
                };
            }
            BridgeStatus::CompositionActive => return PrepareResult::CompositionActive,
            BridgeStatus::Ok => {}
            BridgeStatus::NoLease | BridgeStatus::UserOverride => {
                return self.fail(response_error(&response));
            }
            BridgeStatus::Unknown(response_status) => {
                return self.fail(response_error_for_status(
                    response_status,
                    response.message.as_deref(),
                ));
            }
        }
        let Some(lease_id) = response.lease_id else {
            return self.fail("Squirrel acknowledged acquisition without a lease ID".to_owned());
        };
        self.lease = Some(Lease {
            lease_id,
            ..candidate
        });
        PrepareResult::Ready
    }

    fn release_to_insert(&mut self) -> PrepareResult {
        let Some(lease) = self.lease.clone() else {
            return PrepareResult::Ready;
        };
        let response = match self.request(BridgeMethod::ReleaseToInsert, Some(&lease)) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        match response.status() {
            BridgeStatus::Ok
            | BridgeStatus::StaleSession
            | BridgeStatus::NoActiveSession
            | BridgeStatus::SessionUnavailable
            | BridgeStatus::NoLease
            | BridgeStatus::UserOverride => {
                self.clear_lease();
                PrepareResult::Ready
            }
            BridgeStatus::CompositionActive => self.fail(response_error(&response)),
            BridgeStatus::Unknown(response_status) => self.fail(response_error_for_status(
                response_status,
                response.message.as_deref(),
            )),
        }
    }

    fn request(&mut self, method: BridgeMethod, lease: Option<&Lease>) -> Result<Response, String> {
        let reused_connection = self.stream.is_some();
        self.connect()?;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id.to_string();
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            method,
            session_token: lease.map(|lease| lease.session_token.as_str()),
            session_generation: lease.map(|lease| lease.session_generation),
            lease_id: lease
                .filter(|lease| !lease.lease_id.is_empty())
                .map(|lease| lease.lease_id.as_str()),
        };
        let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        if payload.is_empty() || payload.len() > MAXIMUM_FRAME_SIZE {
            return Err("Squirrel bridge request exceeded the frame limit".to_owned());
        }

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| "Squirrel bridge disconnected".to_owned())?;
        let length = u32::try_from(payload.len())
            .map_err(|_| "Squirrel bridge request exceeded the frame limit".to_owned())?;
        if let Err(error) = stream
            .write_all(&length.to_be_bytes())
            .and_then(|()| stream.write_all(&payload))
        {
            if reused_connection && error.kind() == std::io::ErrorKind::BrokenPipe {
                self.stream.take();
                return self.request(method, lease);
            }
            return Err(format!("failed to write to Squirrel: {error}"));
        }

        let mut header = [0; 4];
        stream
            .read_exact(&mut header)
            .map_err(|error| format!("failed to read from Squirrel: {error}"))?;
        let response_length = usize::try_from(u32::from_be_bytes(header))
            .map_err(|_| "Squirrel returned an invalid frame length".to_owned())?;
        if response_length == 0 || response_length > MAXIMUM_FRAME_SIZE {
            return Err("Squirrel returned an invalid frame length".to_owned());
        }
        let mut response_payload = vec![0; response_length];
        stream
            .read_exact(&mut response_payload)
            .map_err(|error| format!("failed to read from Squirrel: {error}"))?;
        let response: Response = serde_json::from_slice(&response_payload)
            .map_err(|error| format!("Squirrel returned invalid JSON: {error}"))?;
        if response.version != PROTOCOL_VERSION {
            return Err(format!(
                "Squirrel returned protocol version {}",
                response.version
            ));
        }
        if response.request_id != request_id {
            return Err("Squirrel returned a mismatched request ID".to_owned());
        }
        Ok(response)
    }

    fn connect(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }
        let stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| format!("failed to connect to Squirrel: {error}"))?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|error| format!("failed to configure Squirrel read timeout: {error}"))?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|error| format!("failed to configure Squirrel write timeout: {error}"))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn fail(&mut self, message: String) -> PrepareResult {
        log::error!("Squirrel Vim mode bridge faulted: {message}");
        self.disconnect();
        PrepareResult::Faulted(message)
    }

    fn clear_lease(&mut self) {
        self.lease = None;
        self.lease_owner = None;
    }

    fn disconnect(&mut self) {
        self.stream.take();
        self.clear_lease();
    }
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join("squirrel-vim-bridge.sock")
}

fn response_error(response: &Response) -> String {
    response_error_for_status(&response.status, response.message.as_deref())
}

fn response_error_for_status(status: &str, message: Option<&str>) -> String {
    match message {
        Some(message) => format!("Squirrel rejected the request ({status}): {message}"),
        None => format!("Squirrel rejected the request ({status})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        os::unix::net::UnixListener,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
    };

    static NEXT_TEST_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn acquires_and_releases_one_session_lease() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 7,
                "asciiMode": false,
                "compositionActive": false
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 7,
                "leaseId": "lease",
                "asciiMode": true,
                "compositionActive": false
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 7,
                "leaseId": "lease",
                "asciiMode": true,
                "compositionActive": false
            }),
            json!({
                "version": 1,
                "requestId": "4",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 7,
                "asciiMode": false,
                "compositionActive": false
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(
            bridge.lease.as_ref().map(|lease| lease.lease_id.as_str()),
            Some("lease")
        );
        assert_eq!(bridge.acquire_for_owner(2), PrepareResult::Ready);
        assert_eq!(bridge.lease_owner, Some(2));
        assert_eq!(bridge.release_for_owner(1), PrepareResult::Ready);
        assert!(bridge.lease.is_some());
        assert_eq!(bridge.release_for_owner(2), PrepareResult::Ready);
        assert!(bridge.lease.is_none());
        server.join().expect("join test bridge");
    }

    #[test]
    fn composition_keeps_the_existing_mode() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 8,
                "asciiMode": false,
                "compositionActive": true
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "composition_active",
                "sessionToken": "session",
                "sessionGeneration": 8,
                "asciiMode": false,
                "compositionActive": true
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_command(), PrepareResult::CompositionActive);
        assert!(bridge.lease.is_none());
        server.join().expect("join test bridge");
    }
    #[test]
    fn missing_startup_session_is_retryable() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "no_active_session"
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 9,
                "asciiMode": false,
                "compositionActive": false
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 9,
                "leaseId": "lease",
                "asciiMode": true,
                "compositionActive": false
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,

            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Unavailable);
        assert!(bridge.lease_owner.is_none());
        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert!(bridge.lease.is_some());
        server.join().expect("join test bridge");
    }

    #[test]
    fn session_disappearing_during_acquisition_is_unavailable() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 10
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "no_active_session"
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Unavailable);
        assert!(bridge.lease.is_none());
        assert!(bridge.lease_owner.is_none());
        server.join().expect("join test bridge");
    }
    #[test]
    fn retries_one_stale_session_during_acquisition() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "old-session",
                "sessionGeneration": 10
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "stale_session"
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "ok",
                "sessionToken": "new-session",
                "sessionGeneration": 11
            }),
            json!({
                "version": 1,
                "requestId": "4",
                "status": "ok",
                "sessionToken": "new-session",
                "sessionGeneration": 11,
                "leaseId": "lease"
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(
            bridge.lease.as_ref().map(|lease| lease.session_generation),
            Some(11)
        );
        server.join().expect("join test bridge");
    }

    #[test]
    fn stale_release_clears_the_local_lease() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 12
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 12,
                "leaseId": "lease"
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "stale_session"
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(bridge.release_for_owner(1), PrepareResult::Ready);
        assert!(bridge.lease.is_none());
        assert!(bridge.lease_owner.is_none());
        server.join().expect("join test bridge");
    }

    #[test]
    fn session_disappearing_during_release_clears_the_local_lease() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 13
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "ok",
                "sessionToken": "session",
                "sessionGeneration": 13,
                "leaseId": "lease"
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "no_active_session"
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(bridge.release_for_owner(1), PrepareResult::Ready);
        assert!(bridge.lease.is_none());
        assert!(bridge.lease_owner.is_none());
        server.join().expect("join test bridge");
    }

    #[test]
    fn revalidates_a_cached_lease_after_session_change() {
        let (path, server) = test_server(vec![
            json!({
                "version": 1,
                "requestId": "1",
                "status": "ok",
                "sessionToken": "old-session",
                "sessionGeneration": 13
            }),
            json!({
                "version": 1,
                "requestId": "2",
                "status": "ok",
                "sessionToken": "old-session",
                "sessionGeneration": 13,
                "leaseId": "old-lease"
            }),
            json!({
                "version": 1,
                "requestId": "3",
                "status": "ok",
                "sessionToken": "new-session",
                "sessionGeneration": 14
            }),
            json!({
                "version": 1,
                "requestId": "4",
                "status": "ok",
                "sessionToken": "new-session",
                "sessionGeneration": 14,
                "leaseId": "new-lease"
            }),
        ]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            ..Default::default()
        };
        bridge.stream = Some(UnixStream::connect(path).expect("connect test bridge"));

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        assert_eq!(
            bridge.lease.as_ref().map(|lease| lease.session_generation),
            Some(14)
        );
        server.join().expect("join test bridge");
    }

    #[test]
    fn reconnects_after_cached_connection_breaks() {
        let socket_id = NEXT_TEST_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "zed-squirrel-bridge-test-{}-{socket_id}.sock",
            std::process::id(),
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind test bridge");
        let (closed_sender, closed_receiver) = mpsc::sync_channel(0);
        let server_path = path.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept stale test bridge");
            let request = read_test_request(&mut stream);
            assert_eq!(request["method"], "status");
            write_test_response(
                &mut stream,
                &request,
                json!({
                    "version": 1,
                    "status": "no_active_session"
                }),
            );
            stream
                .shutdown(std::net::Shutdown::Both)
                .expect("close stale test bridge");
            drop(stream);
            closed_sender.send(()).expect("signal closed test bridge");

            let (mut stream, _) = listener.accept().expect("accept fresh test bridge");
            let request = read_test_request(&mut stream);
            assert_eq!(request["method"], "status");
            write_test_response(
                &mut stream,
                &request,
                json!({
                    "version": 1,
                    "status": "ok",
                    "sessionToken": "session",
                    "sessionGeneration": 15
                }),
            );
            let request = read_test_request(&mut stream);
            assert_eq!(request["method"], "acquire_command");
            write_test_response(
                &mut stream,
                &request,
                json!({
                    "version": 1,
                    "status": "ok",
                    "sessionToken": "session",
                    "sessionGeneration": 15,
                    "leaseId": "lease"
                }),
            );
            std::fs::remove_file(server_path).expect("remove test socket");
        });
        let mut bridge = SquirrelBridge {
            enabled: true,
            stream: Some(UnixStream::connect(&path).expect("connect test bridge")),
            socket_path: path,
            ..Default::default()
        };

        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Unavailable);
        closed_receiver.recv().expect("wait for closed test bridge");
        let mut byte = [0];
        assert_eq!(
            bridge
                .stream
                .as_mut()
                .expect("cached test bridge")
                .read(&mut byte)
                .expect("observe closed test bridge"),
            0
        );
        assert_eq!(bridge.acquire_for_owner(1), PrepareResult::Ready);
        server.join().expect("join test bridge");
    }

    #[test]
    fn preserves_unknown_rejection_status_and_message() {
        let (path, server) = test_server(vec![json!({
            "version": 1,
            "requestId": "1",
            "status": "permission_denied",
            "message": "bridge access blocked"
        })]);
        let mut bridge = SquirrelBridge {
            enabled: true,
            stream: Some(UnixStream::connect(path).expect("connect test bridge")),
            ..Default::default()
        };

        assert_eq!(
            bridge.acquire_for_owner(1),
            PrepareResult::Faulted(
                "Squirrel rejected the request (permission_denied): bridge access blocked"
                    .to_owned()
            )
        );
        assert!(bridge.stream.is_none());
        assert!(bridge.lease.is_none());
        assert!(bridge.lease_owner.is_none());
        server.join().expect("join test bridge");
    }

    fn read_test_request(stream: &mut UnixStream) -> Value {
        let mut header = [0; 4];
        stream.read_exact(&mut header).expect("read request header");
        let length = u32::from_be_bytes(header) as usize;
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).expect("read request");
        serde_json::from_slice(&payload).expect("decode request")
    }

    fn write_test_response(stream: &mut UnixStream, request: &Value, mut response: Value) {
        response["requestId"] = request["requestId"].clone();
        let payload = serde_json::to_vec(&response).expect("encode response");
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .expect("write response header");
        stream.write_all(&payload).expect("write response");
    }

    fn test_server(responses: Vec<Value>) -> (PathBuf, thread::JoinHandle<()>) {
        let socket_id = NEXT_TEST_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "zed-squirrel-bridge-test-{}-{socket_id}.sock",
            std::process::id(),
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind test bridge");
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let server_path = path.clone();
        let handle = thread::spawn(move || {
            ready_sender.send(()).expect("signal test bridge");
            let (mut stream, _) = listener.accept().expect("accept test bridge");
            for response in responses {
                let request = read_test_request(&mut stream);
                write_test_response(&mut stream, &request, response);
            }
            drop(stream);
            std::fs::remove_file(server_path).expect("remove test socket");
        });
        ready_receiver.recv().expect("wait for test bridge");
        (path, handle)
    }
}
