//! Localhost IPC between the visual editor and a game process it launches.
//!
//! When the editor runs a scene it binds a loopback [`TcpListener`] and passes
//! its address to the game via the `NEOLOVE_EDITOR_IPC` environment variable.
//! The game connects an [`IpcClient`] and streams newline-delimited JSON
//! [`IpcMessage`]s: log lines and periodic live scene snapshots. The editor's
//! logger window reads them into a shared [`LoggerState`].

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::window::{EntitySnapshot, RuntimeLogLine};

/// Cap on retained log lines so a chatty game cannot grow memory without bound.
const MAX_LOG_LINES: usize = 4000;

/// One framed message on the editor<->game channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum IpcMessage {
    #[serde(rename = "log")]
    Log(RuntimeLogLine),
    #[serde(rename = "scene")]
    Scene { entities: Vec<EntitySnapshot> },
}

/// Game-side handle. Serialization and socket writes happen on a background
/// thread so sending never blocks the game's frame loop.
pub struct IpcClient {
    tx: Sender<String>,
}

impl IpcClient {
    /// Connect to the editor's logger listener at `addr` (e.g. `127.0.0.1:54321`).
    /// Returns `None` if the editor is gone; the game then runs normally.
    pub fn connect(addr: &str) -> Option<IpcClient> {
        let stream = TcpStream::connect(addr).ok()?;
        let _ = stream.set_nodelay(true);
        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(stream);
            for line in rx {
                if writer.write_all(line.as_bytes()).is_err()
                    || writer.write_all(b"\n").is_err()
                    || writer.flush().is_err()
                {
                    break;
                }
            }
        });
        Some(IpcClient { tx })
    }

    /// Queue a message for delivery. Drops silently once the editor disconnects.
    pub fn send(&self, message: &IpcMessage) {
        if let Ok(line) = serde_json::to_string(message) {
            let _ = self.tx.send(line);
        }
    }
}

/// Live state the logger window renders, updated by the reader thread.
#[derive(Default)]
pub struct LoggerState {
    pub logs: VecDeque<RuntimeLogLine>,
    pub entities: Vec<EntitySnapshot>,
    /// The game has connected.
    pub connected: bool,
    /// The connection closed (the game exited or the socket dropped).
    pub finished: bool,
}

impl LoggerState {
    pub fn clear_logs(&mut self) {
        self.logs.clear();
    }
}

/// Editor-side listener for a single run. Hold it for the lifetime of the run;
/// dropping it stops accepting (the reader thread ends when the game exits).
pub struct LoggerSession {
    pub state: Arc<Mutex<LoggerState>>,
    /// Address to hand to the game via `NEOLOVE_EDITOR_IPC`.
    pub addr: String,
}

impl LoggerSession {
    /// Bind a loopback listener and spawn a thread that accepts the game's
    /// connection and folds incoming messages into a shared [`LoggerState`].
    pub fn start() -> std::io::Result<LoggerSession> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?.to_string();
        let state = Arc::new(Mutex::new(LoggerState::default()));

        let reader_state = state.clone();
        std::thread::spawn(move || {
            // A run launches exactly one game process, so a single accepted
            // connection is all we need.
            if let Ok((stream, _)) = listener.accept() {
                if let Ok(mut guard) = reader_state.lock() {
                    guard.connected = true;
                }
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(message) = serde_json::from_str::<IpcMessage>(&line) else {
                        continue;
                    };
                    let Ok(mut guard) = reader_state.lock() else {
                        break;
                    };
                    match message {
                        IpcMessage::Log(line) => {
                            guard.logs.push_back(line);
                            while guard.logs.len() > MAX_LOG_LINES {
                                guard.logs.pop_front();
                            }
                        }
                        IpcMessage::Scene { entities } => guard.entities = entities,
                    }
                }
            }
            if let Ok(mut guard) = reader_state.lock() {
                guard.finished = true;
            }
        });

        Ok(LoggerSession { state, addr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::{ComponentSnapshot, EntitySnapshot};
    use std::time::Duration;

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn client_log_and_scene_reach_logger_state() {
        let session = LoggerSession::start().expect("start session");
        let client = IpcClient::connect(&session.addr).expect("connect client");

        client.send(&IpcMessage::Log(RuntimeLogLine {
            level: "info".into(),
            message: "hello from game".into(),
        }));
        client.send(&IpcMessage::Scene {
            entities: vec![EntitySnapshot {
                id: 7,
                name: "Player".into(),
                parent: None,
                x: 1.0,
                y: 2.0,
                rotation: 0.0,
                scale: 1.0,
                enabled: true,
                fields: vec![("size_x".into(), "32".into())],
                components: vec![ComponentSnapshot {
                    name: "Sprite2D".into(),
                    fields: vec![("visible".into(), "true".into())],
                }],
            }],
        });

        let state = session.state.clone();
        assert!(
            wait_until(|| {
                let guard = state.lock().expect("state mutex not poisoned");
                guard.logs.iter().any(|line| line.message == "hello from game")
                    && guard.entities.iter().any(|entity| entity.name == "Player")
            }),
            "log line and scene snapshot should reach the shared logger state"
        );
    }
}
