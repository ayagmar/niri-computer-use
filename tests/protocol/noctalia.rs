//! A fake Noctalia on the fixture's socket that answers `status` with a fixed reply.

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixListener;

use crate::fixture::Fixture;

/// Answers every connection with `reply`, after checking the request is exactly the
/// `status` payload.
pub(crate) fn start(fixture: &Fixture, reply: &'static str) {
    let listener = UnixListener::bind(fixture.noctalia_socket()).unwrap();
    tokio::spawn(async move {
        while let Ok((mut connection, _)) = listener.accept().await {
            let mut request = Vec::new();
            connection.read_to_end(&mut request).await.unwrap();
            assert_eq!(request, b"/\x1estatus");
            connection.write_all(reply.as_bytes()).await.unwrap();
        }
    });
}

pub(crate) const UNLOCKED: &str =
    r#"{"barVisible":true,"panelOpen":false,"activePanelId":null,"locked":false}"#;
pub(crate) const LOCKED: &str =
    r#"{"barVisible":false,"panelOpen":false,"activePanelId":null,"locked":true}"#;
