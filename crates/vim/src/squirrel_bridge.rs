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

#[derive(Default)]
struct SquirrelBridge {
    enabled: bool,
    stream: Option<UnixStream>,
    next_request_id: u64,
    lease: Option<Lease>,
    lease_owner: Option<u64>,
}

#[derive(Clone)]
struct Lease {
    session_token: String,
    session_generation: u64,
    lease_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    version: u8,
    request_id: String,
    method: &'a str,
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
        let status = match self.request("status", None) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        if matches!(
            status.status.as_str(),
            "no_active_session" | "session_unavailable"
        ) {
            self.lease = None;
            self.lease_owner = None;
            return PrepareResult::Unavailable;
        }
        if status.status != "ok" {
            return self.fail(response_error(&status));
        }
        if self.lease.as_ref().is_some_and(|lease| {
            status.session_token.as_deref() == Some(lease.session_token.as_str())
                && status.session_generation == Some(lease.session_generation)
                && status.lease_id.as_deref() == Some(lease.lease_id.as_str())
        }) {
            return PrepareResult::Ready;
        }
        self.lease = None;
        self.lease_owner = None;

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
        let response = match self.request("acquire_command", Some(&candidate)) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        if response.status == "stale_session" {
            return if retry_stale_session {
                self.acquire_command_with_retry(false)
            } else {
                PrepareResult::Unavailable
            };
        }
        if response.status == "composition_active" {
            return PrepareResult::CompositionActive;
        }
        if response.status != "ok" {
            return self.fail(response_error(&response));
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
        let response = match self.request("release_to_insert", Some(&lease)) {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        if matches!(response.status.as_str(), "stale_session" | "no_lease") {
            self.lease = None;
            self.lease_owner = None;
            return PrepareResult::Ready;
        }
        if response.status != "ok" && response.status != "user_override" {
            return self.fail(response_error(&response));
        }
        self.lease = None;
        self.lease_owner = None;
        PrepareResult::Ready
    }

    fn request(&mut self, method: &str, lease: Option<&Lease>) -> Result<Response, String> {
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
        stream
            .write_all(&length.to_be_bytes())
            .and_then(|()| stream.write_all(&payload))
            .map_err(|error| format!("failed to write to Squirrel: {error}"))?;

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
        let stream = UnixStream::connect(socket_path())
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

    fn disconnect(&mut self) {
        self.stream.take();
        self.lease = None;
        self.lease_owner = None;
    }
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join("squirrel-vim-bridge.sock")
}

fn response_error(response: &Response) -> String {
    match &response.message {
        Some(message) => format!(
            "Squirrel rejected the request ({}): {message}",
            response.status
        ),
        None => format!("Squirrel rejected the request ({})", response.status),
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
                let mut header = [0; 4];
                stream.read_exact(&mut header).expect("read request header");
                let length = u32::from_be_bytes(header) as usize;
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).expect("read request");
                let payload = serde_json::to_vec(&response).expect("encode response");
                stream
                    .write_all(&(payload.len() as u32).to_be_bytes())
                    .expect("write response header");
                stream.write_all(&payload).expect("write response");
            }
            drop(stream);
            std::fs::remove_file(server_path).expect("remove test socket");
        });
        ready_receiver.recv().expect("wait for test bridge");
        (path, handle)
    }
}
