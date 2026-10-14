//! Turning the session's accessibility on, for apps that check it before they put
//! themselves on the accessibility bus: Chromium, Electron and CEF apps read
//! `org.a11y.Status.IsEnabled` on the session bus once, when they start. The server never
//! writes that property. It registers an event listener with the registry, as a screen
//! reader does, on a connection of its own: at-spi2-core 2.61 and later keep `IsEnabled`
//! true while any client has a listener registered, and turn it off once none is left. The
//! listener lasts as long as the connection, so the session's accessibility goes back off
//! when the server ends, however it ends, unless another listener, such as a screen
//! reader's or another server's, keeps it on. A server that finds it on through others'
//! listeners adds its own, so it stays on while the server needs it. An older at-spi2-core
//! doesn't turn it on for a listener; the server then lets go and says so.

use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;
use zbus::Connection;

use super::{CALL, connect, upstream, within};
use crate::error::{ErrorName, ToolError};

/// The registry's object, which takes listeners.
const REGISTRY: &str = "/org/a11y/atspi/registry";
/// The event the listener asks for: one an app sends seldom, as only a window's activation
/// sends it.
const EVENT: &str = "window:activate";
/// How long the bus launcher has to turn `IsEnabled` on after the listener is registered.
const TURNING_ON: Duration = Duration::from_millis(500);
/// How often `IsEnabled` is read while it turns on.
const POLL: Duration = Duration::from_millis(20);

/// The server's listener, and whether it turned the session's accessibility on.
#[derive(Debug)]
pub(crate) struct Switch {
    /// The session bus's address, where `org.a11y.Status` is.
    session_bus: Option<String>,
    /// The accessibility bus's address, where the registry is.
    a11y_bus: String,
    held: tokio::sync::Mutex<Held>,
}

/// What the server has done to the session's accessibility.
#[derive(Debug, Default)]
enum Held {
    /// Nothing.
    #[default]
    Off,
    /// Its listener holds accessibility on for as long as the connection lasts.
    On { _listener: Connection },
    /// It registered a listener, and the bus launcher didn't turn accessibility on: why.
    Unsupported(String),
    /// Something turned accessibility off while its listener held it on, so it let the
    /// listener go and doesn't register another.
    LetGo,
}

/// What `status` reports of the switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Report {
    /// The server hasn't turned the session's accessibility on.
    Off,
    /// The server's listener holds it on.
    On,
    /// The server is turning it on now.
    TurningOn,
    /// The bus launcher doesn't turn it on for a listener.
    Unsupported,
    /// Something turned it off while the server held it on, and the server let go.
    LetGo,
}

/// What `listen` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Turned {
    /// The listener is registered and `IsEnabled` is true.
    On,
    /// The server's listener already held it on.
    AlreadyOn,
    /// The listener didn't turn it on, as an at-spi2-core before 2.61 doesn't: why.
    Unsupported(String),
    /// Another call was still turning it on when this one's time ran out.
    Busy,
    /// Something turned it off while the server's listener held it on; the listener is let
    /// go, now or before.
    LetGo,
    /// This call's time ran out before the launcher's `TURNING_ON` did, with `IsEnabled`
    /// still false, so the listener was let go for a later call to try again.
    Late,
    /// `IsEnabled` couldn't be read after the listener registered, so the listener was let
    /// go for a later call to try again: why.
    Unread(String),
}

/// How `IsEnabled` read while the server waited for it.
#[derive(Debug, PartialEq, Eq)]
enum Waited {
    On,
    /// False for the whole of `TURNING_ON`.
    Off,
    /// False until the call's deadline, before `TURNING_ON` ran out.
    Cut,
}

impl Switch {
    pub(crate) const fn new(session_bus: Option<String>, a11y_bus: String) -> Self {
        Self {
            session_bus,
            a11y_bus,
            held: tokio::sync::Mutex::const_new(Held::Off),
        }
    }

    /// Whether the session's accessibility is on: `org.a11y.Status.IsEnabled`, read by
    /// `deadline`.
    pub(crate) async fn session_enabled(&self, deadline: Instant) -> Result<bool, ToolError> {
        let session_bus = self.session_bus.as_deref().ok_or_else(|| {
            ToolError::new(
                ErrorName::UpstreamError,
                "neither DBUS_SESSION_BUS_ADDRESS nor XDG_RUNTIME_DIR is set",
            )
        })?;
        let connection = within(
            left(deadline),
            "connect to the session bus",
            connect(session_bus),
        )
        .await?;
        is_enabled(&connection, deadline).await
    }

    /// Whether any client has a listener registered with the registry, by `deadline`.
    pub(crate) async fn listened(&self, deadline: Instant) -> Result<bool, ToolError> {
        let connection = within(
            left(deadline),
            "connect to the accessibility bus",
            connect(&self.a11y_bus),
        )
        .await?;
        let what = "org.a11y.atspi.Registry.GetRegisteredEvents";
        let reply = within(
            left(deadline),
            what,
            connection.call_method(
                Some("org.a11y.atspi.Registry"),
                REGISTRY,
                Some("org.a11y.atspi.Registry"),
                "GetRegisteredEvents",
                &(),
            ),
        )
        .await?;
        let events: Vec<(String, String)> = reply
            .body()
            .deserialize()
            .map_err(|error| upstream(what, &error))?;
        Ok(!events.is_empty())
    }

    /// Registers the listener, unless the server's already holds accessibility on, and
    /// waits for `IsEnabled` to turn true, all by `deadline`, waiting for another call
    /// turning it on no longer. A launcher that doesn't turn it on is remembered, and the
    /// listener let go. So is a listener that holds it on while it reads off, as `enabled`
    /// says it did just now: something turned it off over the listener, and the launcher
    /// would turn it back on the next time any listener comes or goes.
    pub(crate) async fn listen(
        &self,
        deadline: Instant,
        enabled: bool,
    ) -> Result<Turned, ToolError> {
        let Ok(mut held) = tokio::time::timeout_at(deadline, self.held.lock()).await else {
            return Ok(Turned::Busy);
        };
        match &*held {
            Held::On { .. } if enabled => return Ok(Turned::AlreadyOn),
            Held::On { .. } => {
                *held = Held::LetGo;
                return Ok(Turned::LetGo);
            }
            Held::LetGo => return Ok(Turned::LetGo),
            Held::Unsupported(why) => return Ok(Turned::Unsupported(why.clone())),
            Held::Off => {}
        }
        let listener = within(
            left(deadline),
            "connect to the accessibility bus",
            connect(&self.a11y_bus),
        )
        .await?;
        within(
            left(deadline),
            "org.a11y.atspi.Registry.RegisterEvent",
            listener.call_method(
                Some("org.a11y.atspi.Registry"),
                REGISTRY,
                Some("org.a11y.atspi.Registry"),
                "RegisterEvent",
                &(EVENT,),
            ),
        )
        .await?;
        let waited = match self.turned_on(deadline).await {
            Ok(waited) => waited,
            Err(error) => return Ok(Turned::Unread(error.detail)),
        };
        match waited {
            Waited::On => {
                *held = Held::On {
                    _listener: listener,
                };
                drop(held);
                return Ok(Turned::On);
            }
            Waited::Cut => return Ok(Turned::Late),
            Waited::Off => {}
        }
        let why = format!(
            "org.a11y.Status IsEnabled stayed false {} ms after the server registered a listener; at-spi2-core before 2.61 doesn't turn it on for one",
            TURNING_ON.as_millis()
        );
        *held = Held::Unsupported(why.clone());
        drop(held);
        Ok(Turned::Unsupported(why))
    }

    /// Whether `IsEnabled` turns true within `TURNING_ON`, or by `deadline` when that
    /// comes first. Each read has until `deadline`, so one begun near the end of the wait
    /// still gets its answer.
    async fn turned_on(&self, deadline: Instant) -> Result<Waited, ToolError> {
        let full = Instant::now() + TURNING_ON;
        let until = deadline.min(full);
        let off = if until < full {
            Waited::Cut
        } else {
            Waited::Off
        };
        loop {
            if self.session_enabled(deadline).await? {
                return Ok(Waited::On);
            }
            if Instant::now() + POLL >= until {
                return Ok(off);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    pub(crate) fn report(&self) -> Report {
        match self.held.try_lock().as_deref() {
            Ok(Held::Off) => Report::Off,
            Ok(Held::On { .. }) => Report::On,
            Ok(Held::Unsupported(_)) => Report::Unsupported,
            Ok(Held::LetGo) => Report::LetGo,
            Err(_) => Report::TurningOn,
        }
    }
}

/// `IsEnabled` on the session bus `connection`, read by `deadline`.
async fn is_enabled(connection: &Connection, deadline: Instant) -> Result<bool, ToolError> {
    let what = "org.a11y.Status IsEnabled";
    let reply = within(
        left(deadline),
        what,
        connection.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.a11y.Status", "IsEnabled"),
        ),
    )
    .await?;
    let value: zbus::zvariant::OwnedValue = reply
        .body()
        .deserialize()
        .map_err(|error| upstream(what, &error))?;
    bool::try_from(value).map_err(|error| upstream(what, &zbus::Error::Variant(error)))
}

/// What is left until `deadline`, at most one call's deadline.
fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now()).min(CALL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_call_waiting_for_another_turning_it_on_ends_by_its_deadline() {
        let switch = Switch::new(None, "unix:path=/nonexistent".to_owned());
        // Another call, stuck registering its listener.
        let _turning = switch.held.lock().await;
        let deadline = Instant::now() + Duration::from_millis(100);
        let turned = tokio::time::timeout(Duration::from_secs(2), switch.listen(deadline, false))
            .await
            .expect("the call waited past its deadline");
        assert_eq!(turned.unwrap(), Turned::Busy);
    }
}
