//! A fake Noctalia on the fixture's socket. It answers `status` with a reply the test
//! sets, and `panel-open` and `panel-close` the way Noctalia 5.2.1 does, keeping the open
//! panel in its `activePanelId`.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixListener;

use crate::fixture::Fixture;

#[derive(Debug)]
struct State {
    status: &'static str,
    active_panel: Option<String>,
    /// Whether panel commands change the open panel, or only answer `ok`.
    panels_follow: bool,
    /// Every command received, after the `/\x1e` prefix.
    commands: Vec<String>,
}

impl State {
    fn reply(&mut self, command: &str) -> String {
        match command.split_once(' ') {
            None if command == "status" => status(self.status, self.active_panel.as_deref()),
            Some(("panel-open", panel)) => {
                if self.panels_follow {
                    self.active_panel = Some(panel.to_owned());
                }
                "ok\n".to_owned()
            }
            Some(("panel-close", panel)) => {
                if self.panels_follow && self.active_panel.as_deref() == Some(panel) {
                    self.active_panel = None;
                }
                "ok\n".to_owned()
            }
            _ => format!("error: unknown command {command:?}\n"),
        }
    }
}

/// The fake's state, which the test changes.
#[derive(Debug, Clone)]
pub(crate) struct Reply(Arc<Mutex<State>>);

impl Reply {
    /// The `status` reply from now on, with the open panel the fake keeps.
    pub(crate) fn set(&self, reply: &'static str) {
        self.0.lock().unwrap().status = reply;
    }

    /// With false, panel commands are answered `ok` and change nothing.
    pub(crate) fn panels_follow(&self, follow: bool) {
        self.0.lock().unwrap().panels_follow = follow;
    }

    /// The panel commands received so far.
    pub(crate) fn panel_commands(&self) -> Vec<String> {
        let state = self.0.lock().unwrap();
        state
            .commands
            .iter()
            .filter(|command| *command != "status")
            .cloned()
            .collect()
    }

    fn answer(&self, command: &str) -> String {
        let mut state = self.0.lock().unwrap();
        state.commands.push(command.to_owned());
        let reply = state.reply(command);
        drop(state);
        reply
    }
}

/// Answers every connection, after checking the request has the `/\x1e` prefix.
pub(crate) fn start(fixture: &Fixture, reply: &'static str) -> Reply {
    let listener = UnixListener::bind(fixture.noctalia_socket()).unwrap();
    let current = Reply(Arc::new(Mutex::new(State {
        status: reply,
        active_panel: None,
        panels_follow: true,
        commands: Vec::new(),
    })));
    let state = current.clone();
    tokio::spawn(async move {
        while let Ok((mut connection, _)) = listener.accept().await {
            let mut request = Vec::new();
            connection.read_to_end(&mut request).await.unwrap();
            let command = request.strip_prefix(b"/\x1e").unwrap();
            let command = String::from_utf8(command.to_vec()).unwrap();
            let answer = state.answer(&command);
            connection.write_all(answer.as_bytes()).await.unwrap();
        }
    });
    current
}

/// `reply` with `activePanelId` and `panelOpen` set from the open panel.
fn status(reply: &str, active_panel: Option<&str>) -> String {
    let mut status: Value = serde_json::from_str(reply).unwrap();
    status["activePanelId"] = active_panel.map_or(Value::Null, Value::from);
    status["panelOpen"] = Value::Bool(active_panel.is_some());
    status.to_string()
}

pub(crate) const UNLOCKED: &str =
    r#"{"barVisible":true,"panelOpen":false,"activePanelId":null,"locked":false}"#;
pub(crate) const LOCKED: &str =
    r#"{"barVisible":false,"panelOpen":false,"activePanelId":null,"locked":true}"#;
