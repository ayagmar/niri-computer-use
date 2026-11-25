//! A desk's accessibility bus. The bus launcher claims `org.a11y.Bus` on the desk's session
//! bus, which has no service directories, so the supervisor starts the registry daemon
//! itself. Then it turns the desk's accessibility on before any app starts, since
//! Chromium-family apps read `org.a11y.Status.IsEnabled` once, when they start: it sets
//! the property, for an at-spi2-core before 2.61, and registers a listener with the
//! registry, which 2.61 and later keep it on for, on a connection held as long as the desk.

use std::time::Duration;

use tokio::time::Instant;
use zbus::Connection;

/// How often a bus name is looked for while its daemon starts.
const POLL: Duration = Duration::from_millis(50);
const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
const REGISTRY: &str = "/org/a11y/atspi/registry";

/// Connects to the bus at `address` by `deadline`.
pub(crate) async fn connect(address: &str, deadline: Instant) -> Result<Connection, String> {
    let connecting = zbus::connection::Builder::address(address).build();
    tokio::time::timeout_at(deadline, connecting)
        .await
        .map_err(|_| format!("no connection to the bus at {address} by the step's deadline"))?
        .map_err(|error| format!("connect to the bus at {address}: {error}"))
}

/// Waits until `name` has an owner on `bus`, by `deadline`.
pub(crate) async fn owned(bus: &Connection, name: &str, deadline: Instant) -> Result<(), String> {
    loop {
        let reply = tokio::time::timeout_at(
            deadline,
            bus.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(name,),
            ),
        )
        .await
        .map_err(|_| format!("{name} had no owner by the step's deadline"))?
        .map_err(|error| format!("ask whether {name} has an owner: {error}"))?;
        let has_owner: bool = reply
            .body()
            .deserialize()
            .map_err(|error| format!("read whether {name} has an owner: {error}"))?;
        if has_owner {
            return Ok(());
        }
        if Instant::now() + POLL >= deadline {
            return Err(format!("{name} had no owner by the step's deadline"));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The accessibility bus's address, from the launcher on the desk's session bus.
pub(crate) async fn address(session: &Connection, deadline: Instant) -> Result<String, String> {
    owned(session, "org.a11y.Bus", deadline).await?;
    let reply = tokio::time::timeout_at(
        deadline,
        session.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.a11y.Bus"),
            "GetAddress",
            &(),
        ),
    )
    .await
    .map_err(|_| "org.a11y.Bus.GetAddress didn't answer by the step's deadline".to_owned())?
    .map_err(|error| format!("org.a11y.Bus.GetAddress: {error}"))?;
    reply
        .body()
        .deserialize()
        .map_err(|error| format!("read org.a11y.Bus.GetAddress's reply: {error}"))
}

/// Turns the desk's accessibility on once the registry runs on `accessibility`: the
/// returned connection holds the listener that keeps it on.
pub(crate) async fn turn_on(
    session: &Connection,
    accessibility_bus: &str,
    deadline: Instant,
) -> Result<Connection, String> {
    let listener = connect(accessibility_bus, deadline).await?;
    owned(&listener, REGISTRY_NAME, deadline).await?;
    let late = |what: &str| format!("{what} didn't answer by the step's deadline");
    tokio::time::timeout_at(
        deadline,
        listener.call_method(
            Some(REGISTRY_NAME),
            REGISTRY,
            Some(REGISTRY_NAME),
            "RegisterEvent",
            &("window:activate",),
        ),
    )
    .await
    .map_err(|_| late("org.a11y.atspi.Registry.RegisterEvent"))?
    .map_err(|error| format!("org.a11y.atspi.Registry.RegisterEvent: {error}"))?;
    let value = zbus::Variant::from(true);
    tokio::time::timeout_at(
        deadline,
        session.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &("org.a11y.Status", "IsEnabled", value),
        ),
    )
    .await
    .map_err(|_| late("setting org.a11y.Status IsEnabled"))?
    .map_err(|error| format!("set org.a11y.Status IsEnabled: {error}"))?;
    if !enabled(session, deadline).await? {
        return Err("org.a11y.Status IsEnabled stayed false after the desk set it and registered a listener".to_owned());
    }
    Ok(listener)
}

async fn enabled(session: &Connection, deadline: Instant) -> Result<bool, String> {
    let reply = tokio::time::timeout_at(
        deadline,
        session.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.a11y.Status", "IsEnabled"),
        ),
    )
    .await
    .map_err(|_| {
        "reading org.a11y.Status IsEnabled didn't answer by the step's deadline".to_owned()
    })?
    .map_err(|error| format!("read org.a11y.Status IsEnabled: {error}"))?;
    let value: zbus::OwnedVariant = reply
        .body()
        .deserialize()
        .map_err(|error| format!("read org.a11y.Status IsEnabled: {error}"))?;
    bool::try_from(value)
        .map_err(|error| format!("org.a11y.Status IsEnabled isn't a boolean: {error}"))
}
