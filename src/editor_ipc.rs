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
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::window::{EntitySnapshot, RuntimeLogLine};

/// The latest framebuffer and timing sample produced by an editor-embedded
/// runtime. PNG keeps the localhost stream bounded enough for interactive use
/// while preserving exact RGBA output for parity captures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeFrame {
    pub serial: u64,
    pub width: u32,
    pub height: u32,
    pub png_base64: String,
    pub backend: String,
    pub fps: f32,
    pub update_ms: f32,
    pub render_ms: f32,
    pub draw_calls: u32,
    pub triangles: u64,
}

/// One complete editor Game View input sample. The runtime diffs the held-key
/// and held-button lists against its own platform state so pressed/released
/// edges follow the same `begin_frame` lifecycle as native window input.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeInputSnapshot {
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub mouse_buttons: Vec<String>,
    pub keys: Vec<String>,
    pub wheel_x: f32,
    pub wheel_y: f32,
    pub text: String,
}

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
    /// Authoritative scene state immediately after runtime startup and before
    /// the first update. Retained separately so scripts and physics cannot
    /// create false authored/runtime serialization diffs.
    #[serde(rename = "initial_scene")]
    InitialScene { entities: Vec<EntitySnapshot> },
    #[serde(rename = "frame")]
    Frame(RuntimeFrame),
}

/// Editor-to-runtime play controls. These are handled by the real desktop
/// runtime loop, so pause and step never run a second editor-only simulation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum IpcCommand {
    Pause,
    Resume,
    Step,
    Stop,
    Input { snapshot: RuntimeInputSnapshot },
    Resize { width: u32, height: u32 },
}

/// Game-side handle. Serialization and socket writes happen on a background
/// thread so sending never blocks the game's frame loop.
pub struct IpcClient {
    tx: Sender<String>,
    commands: Receiver<IpcCommand>,
}

impl IpcClient {
    /// Connect to the editor's logger listener at `addr` (e.g. `127.0.0.1:54321`).
    /// Returns `None` if the editor is gone; the game then runs normally.
    pub fn connect(addr: &str) -> Option<IpcClient> {
        let stream = TcpStream::connect(addr).ok()?;
        let _ = stream.set_nodelay(true);
        let command_stream = stream.try_clone().ok()?;
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
        let (command_tx, commands) = channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(command_stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(command) = serde_json::from_str::<IpcCommand>(&line) else {
                    continue;
                };
                if command_tx.send(command).is_err() {
                    break;
                }
            }
        });
        Some(IpcClient { tx, commands })
    }

    /// Queue a message for delivery. Drops silently once the editor disconnects.
    pub fn send(&self, message: &IpcMessage) {
        if let Ok(line) = serde_json::to_string(message) {
            let _ = self.tx.send(line);
        }
    }

    /// Drain play controls without ever blocking the runtime frame loop.
    pub fn drain_commands(&self) -> Vec<IpcCommand> {
        self.commands.try_iter().collect()
    }
}

/// Live state the logger window renders, updated by the reader thread.
#[derive(Default)]
pub struct LoggerState {
    pub logs: VecDeque<RuntimeLogLine>,
    pub entities: Vec<EntitySnapshot>,
    pub initial_entities: Option<Vec<EntitySnapshot>>,
    pub latest_frame: Option<RuntimeFrame>,
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
    command_tx: Sender<IpcCommand>,
}

impl LoggerSession {
    /// Bind a loopback listener and spawn a thread that accepts the game's
    /// connection and folds incoming messages into a shared [`LoggerState`].
    pub fn start() -> std::io::Result<LoggerSession> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?.to_string();
        let state = Arc::new(Mutex::new(LoggerState::default()));
        let (command_tx, command_rx) = channel::<IpcCommand>();

        let reader_state = state.clone();
        std::thread::spawn(move || {
            // A run launches exactly one game process, so a single accepted
            // connection is all we need.
            if let Ok((stream, _)) = listener.accept() {
                if let Ok(command_stream) = stream.try_clone() {
                    std::thread::spawn(move || write_commands(command_stream, command_rx));
                }
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
                        IpcMessage::InitialScene { entities } => {
                            if guard.initial_entities.is_none() {
                                guard.initial_entities = Some(entities.clone());
                            }
                            guard.entities = entities;
                        }
                        IpcMessage::Frame(frame) => guard.latest_frame = Some(frame),
                    }
                }
            }
            if let Ok(mut guard) = reader_state.lock() {
                guard.finished = true;
            }
        });

        Ok(LoggerSession {
            state,
            addr,
            command_tx,
        })
    }

    pub fn command_sender(&self) -> Sender<IpcCommand> {
        self.command_tx.clone()
    }
}

fn write_commands(stream: TcpStream, commands: Receiver<IpcCommand>) {
    let mut writer = BufWriter::new(stream);
    for command in commands {
        let Ok(line) = serde_json::to_string(&command) else {
            continue;
        };
        if writer.write_all(line.as_bytes()).is_err()
            || writer.write_all(b"\n").is_err()
            || writer.flush().is_err()
        {
            break;
        }
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
            ..RuntimeLogLine::default()
        }));
        client.send(&IpcMessage::InitialScene {
            entities: vec![EntitySnapshot {
                id: 7,
                source_id: Some(42),
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
                    source_index: Some(0),
                    source_key: Some("core:Sprite2D".into()),
                    fields: vec![("visible".into(), "true".into())],
                }],
            }],
        });

        let state = session.state.clone();
        assert!(
            wait_until(|| {
                let guard = state.lock().expect("state mutex not poisoned");
                guard
                    .logs
                    .iter()
                    .any(|line| line.message == "hello from game")
                    && guard.entities.iter().any(|entity| entity.name == "Player")
                    && guard
                        .initial_entities
                        .as_ref()
                        .is_some_and(|entities| entities.iter().any(|entity| {
                            entity.source_id == Some(42) && entity.name == "Player"
                        }))
            }),
            "log line and scene snapshot should reach the shared logger state"
        );
    }

    #[test]
    fn embedded_frame_reaches_shared_state_without_growing_a_queue() {
        let session = LoggerSession::start().expect("start session");
        let client = IpcClient::connect(&session.addr).expect("connect client");
        for serial in 1..=3 {
            client.send(&IpcMessage::Frame(RuntimeFrame {
                serial,
                width: 320,
                height: 180,
                png_base64: format!("frame-{serial}"),
                backend: "software-embedded".into(),
                fps: 60.0,
                update_ms: serial as f32,
                render_ms: serial as f32 * 2.0,
                draw_calls: 4,
                triangles: 120,
            }));
        }
        let state = session.state.clone();
        assert!(wait_until(|| {
            state
                .lock()
                .expect("state")
                .latest_frame
                .as_ref()
                .is_some_and(|frame| frame.serial == 3)
        }));
        let guard = state.lock().expect("state");
        let frame = guard.latest_frame.as_ref().expect("latest frame");
        assert_eq!(frame.png_base64, "frame-3");
        assert_eq!(frame.render_ms, 6.0);
    }

    #[test]
    fn editor_play_commands_reach_runtime_client_in_order() {
        let session = LoggerSession::start().expect("start session");
        let commands = session.command_sender();
        let client = IpcClient::connect(&session.addr).expect("connect client");
        for command in [
            IpcCommand::Pause,
            IpcCommand::Step,
            IpcCommand::Resume,
            IpcCommand::Stop,
            IpcCommand::Input {
                snapshot: RuntimeInputSnapshot {
                    mouse_x: 12.0,
                    mouse_y: 34.0,
                    mouse_buttons: vec!["left".into()],
                    keys: vec!["w".into()],
                    wheel_x: 0.0,
                    wheel_y: 1.0,
                    text: "x".into(),
                },
            },
            IpcCommand::Resize {
                width: 640,
                height: 360,
            },
        ] {
            commands.send(command).expect("queue command");
        }
        let mut received = Vec::new();
        assert!(
            wait_until(|| {
                received.extend(client.drain_commands());
                received.len() == 6
            }),
            "runtime should receive every queued command"
        );
        assert_eq!(
            received,
            [
                IpcCommand::Pause,
                IpcCommand::Step,
                IpcCommand::Resume,
                IpcCommand::Stop,
                IpcCommand::Input {
                    snapshot: RuntimeInputSnapshot {
                        mouse_x: 12.0,
                        mouse_y: 34.0,
                        mouse_buttons: vec!["left".into()],
                        keys: vec!["w".into()],
                        wheel_x: 0.0,
                        wheel_y: 1.0,
                        text: "x".into(),
                    },
                },
                IpcCommand::Resize {
                    width: 640,
                    height: 360,
                },
            ]
        );
    }
}
