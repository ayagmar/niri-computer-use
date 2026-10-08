//! A fake Noctalia on the fixture's socket that answers `status` with a reply the test
//! sets.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixListener;

use crate::fixture::Fixture;

/// The reply the fake gives from now on.
#[derive(Debug, Clone)]
pub(crate) struct Reply(Arc<Mutex<&'static str>>);

impl Reply {
    pub(crate) fn set(&self, reply: &'static str) {
        *self.0.lock().unwrap() = reply;
    }
}

/// Answers every connection with `reply`, after checking the request is exactly the
/// `status` payload.
pub(crate) fn start(fixture: &Fixture, reply: &'static str) -> Reply {
    let listener = UnixListener::bind(fixture.noctalia_socket()).unwrap();
    let current = Reply(Arc::new(Mutex::new(reply)));
    let replies = current.clone();
    tokio::spawn(async move {
        while let Ok((mut connection, _)) = listener.accept().await {
            let mut request = Vec::new();
            connection.read_to_end(&mut request).await.unwrap();
            assert_eq!(request, b"/\x1estatus");
            let answer = *replies.0.lock().unwrap();
            connection.write_all(answer.as_bytes()).await.unwrap();
        }
    });
    current
}

pub(crate) const UNLOCKED: &str =
    r#"{"barVisible":true,"panelOpen":false,"activePanelId":null,"locked":false}"#;
pub(crate) const LOCKED: &str =
    r#"{"barVisible":false,"panelOpen":false,"activePanelId":null,"locked":true}"#;
