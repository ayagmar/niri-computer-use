//! The clipboard through wlr data-control, on a Wayland connection of its own to niri:
//! saving every MIME type of the current selection, taking the selection with a source of
//! our own, and the reads of that source. Every round trip and every transfer has a
//! deadline; waiting for the next event has none, because a source is served for as long
//! as it holds the selection.

use std::collections::VecDeque;
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, Interest};
use tokio::net::unix::pipe;
use tokio::time::Instant;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    self, ZwlrDataControlDeviceV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};

use super::wayland::{DEADLINE, Synced, connect, dispatch_until, roundtrip, upstream};
use crate::error::{ErrorName, ToolError};

/// A selection's contents: each MIME type with its bytes, in the order it was offered.
pub(crate) type Contents = Vec<(String, Arc<[u8]>)>;

/// A selection as saved: its contents, `None` when nothing was selected, and which of
/// niri's announcements it was.
#[derive(Debug)]
pub(crate) struct Saved {
    pub(crate) contents: Option<Contents>,
    announcement: u64,
}

impl Saved {
    /// Whether this is still the selection when niri has made `announced` announcements:
    /// none came after the one it was saved from.
    const fn still_current(&self, announced: u64) -> bool {
        announced == self.announcement
    }
}

/// A source this connection made, numbered in the order it was made.
pub(crate) type SourceId = u32;

#[derive(Debug)]
pub(crate) enum Event {
    /// A client reads `mime` from one of our sources through `fd`.
    Send {
        source: SourceId,
        mime: String,
        fd: OwnedFd,
    },
    /// Another client took the selection from one of our sources.
    Cancelled(SourceId),
}

/// The selection niri last announced.
#[derive(Debug, Default, Clone)]
enum Current {
    #[default]
    Unannounced,
    Empty,
    Offer(ZwlrDataControlOfferV1),
}

#[derive(Debug, Default)]
struct State {
    /// The registry's globals: name, interface and version.
    globals: Vec<(u32, String, u32)>,
    /// The offers niri introduced, with the MIME types each has announced.
    offers: Vec<(ZwlrDataControlOfferV1, Vec<String>)>,
    selection: Current,
    /// How many selections niri has announced.
    announcements: u64,
    /// Whether niri has ended the data device.
    finished: bool,
    events: VecDeque<Event>,
    synced: bool,
}

#[derive(Debug)]
pub(crate) struct Selection {
    queue: EventQueue<State>,
    state: State,
    readable: AsyncFd<OwnedFd>,
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    sources: SourceId,
    connection: Connection,
}

impl Selection {
    /// Connects to the Wayland display at `display`, checks that `niri_pid` serves it, and
    /// reads the current selection's MIME types.
    pub(crate) async fn bind(display: &Path, niri_pid: u32) -> Result<Self, ToolError> {
        let deadline = Instant::now() + DEADLINE;
        let connection = connect(display, niri_pid).await?;
        let fd = connection
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|error| upstream(&format!("copy the Wayland socket: {error}")))?;
        let readable = AsyncFd::with_interest(fd, Interest::READABLE)
            .map_err(|error| upstream(&format!("watch the Wayland socket: {error}")))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        let registry = connection.display().get_registry(&handle, ());
        let mut state = State::default();
        roundtrip(&connection, &mut queue, &mut state, &readable, deadline).await?;
        let find = |interface: &str| {
            state
                .globals
                .iter()
                .find(|(_, name, _)| name == interface)
                .map(|(id, ..)| *id)
                .ok_or_else(|| upstream(&format!("niri offers no {interface}")))
        };
        let seat = registry.bind::<WlSeat, _, _>(find("wl_seat")?, 1, &handle, ());
        // Version 1: the primary selection stays out of this.
        let manager = registry.bind::<ZwlrDataControlManagerV1, _, _>(
            find("zwlr_data_control_manager_v1")?,
            1,
            &handle,
            (),
        );
        let device = manager.get_data_device(&seat, &handle, ());
        roundtrip(&connection, &mut queue, &mut state, &readable, deadline).await?;
        if matches!(state.selection, Current::Unannounced) {
            return Err(upstream("niri didn't announce the current selection"));
        }
        Ok(Self {
            queue,
            state,
            readable,
            manager,
            device,
            sources: 0,
            connection,
        })
    }

    /// Reads every MIME type of the current selection, within `DEADLINE` and `max` bytes in
    /// all. A selection niri announces meanwhile isn't seen; `unchanged_since` tells.
    pub(crate) async fn save(&self, max: usize) -> Result<Saved, ToolError> {
        let announcement = self.state.announcements;
        let contents = self.contents(max).await?;
        Ok(Saved {
            contents,
            announcement,
        })
    }

    /// Whether `saved` is still the selection, once every announcement niri sent before
    /// now is handled.
    pub(crate) async fn unchanged_since(&mut self, saved: &Saved) -> Result<bool, ToolError> {
        self.sync().await?;
        Ok(saved.still_current(self.state.announcements))
    }

    /// The current selection's contents; `None` when nothing is selected.
    async fn contents(&self, max: usize) -> Result<Option<Contents>, ToolError> {
        let deadline = Instant::now() + DEADLINE;
        let Current::Offer(offer) = self.state.selection.clone() else {
            return Ok(None);
        };
        let types = self
            .state
            .offers
            .iter()
            .find(|(known, _)| *known == offer)
            .map(|(_, types)| types.clone())
            .unwrap_or_default();
        if types.is_empty() {
            return Ok(None);
        }
        let mut contents = Contents::new();
        let mut left = max;
        for mime in types {
            let bytes = self.receive(&offer, &mime, left, deadline).await?;
            left -= bytes.len();
            contents.push((mime, bytes.into()));
        }
        Ok(Some(contents))
    }

    async fn receive(
        &self,
        offer: &ZwlrDataControlOfferV1,
        mime: &str,
        max: usize,
        deadline: Instant,
    ) -> Result<Vec<u8>, ToolError> {
        let (reader, writer) =
            std::io::pipe().map_err(|error| upstream(&format!("make a pipe: {error}")))?;
        offer.receive(mime.to_owned(), writer.as_fd());
        self.queue
            .flush()
            .map_err(|error| upstream(&format!("niri's Wayland display: {error}")))?;
        // Only the selection's owner may hold a write end now.
        drop(writer);
        let reader = pipe::Receiver::from_owned_fd(reader.into())
            .map_err(|error| upstream(&format!("read the selection: {error}")))?;
        let mut bytes = Vec::new();
        let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
        tokio::time::timeout_at(deadline, reader.take(limit).read_to_end(&mut bytes))
            .await
            .map_err(|_| {
                ToolError::new(
                    ErrorName::DeadlineExceeded,
                    format!("the selection's owner didn't send {mime} within {DEADLINE:?}"),
                )
            })?
            .map_err(|error| upstream(&format!("read the selection's {mime}: {error}")))?;
        if bytes.len() > max {
            return Err(upstream("the selection is larger than the cap"));
        }
        Ok(bytes)
    }

    /// Takes the selection with a source offering `types`, and waits until niri has it.
    pub(crate) async fn offer(&mut self, types: &[&str]) -> Result<SourceId, ToolError> {
        self.sources += 1;
        let source = self
            .manager
            .create_data_source(&self.queue.handle(), self.sources);
        for mime in types {
            source.offer((*mime).to_owned());
        }
        self.device.set_selection(Some(&source));
        self.sync().await?;
        Ok(self.sources)
    }

    /// Leaves the selection empty, and waits until niri has.
    pub(crate) async fn clear(&mut self) -> Result<(), ToolError> {
        self.device.set_selection(None);
        self.sync().await
    }

    /// The next read of, or end of, one of our sources. A data device niri ended is an
    /// error.
    pub(crate) async fn next(&mut self) -> Result<Event, ToolError> {
        dispatch_until(
            &mut self.queue,
            &mut self.state,
            &self.readable,
            None,
            |state| state.finished || !state.events.is_empty(),
        )
        .await?;
        self.state
            .events
            .pop_front()
            .ok_or_else(|| upstream("niri ended the data-control device"))
    }

    /// The reads and ends of our sources that niri sent before now, in order.
    pub(crate) async fn pending(&mut self) -> Result<Vec<Event>, ToolError> {
        self.sync().await?;
        Ok(self.state.events.drain(..).collect())
    }

    async fn sync(&mut self) -> Result<(), ToolError> {
        let deadline = Instant::now() + DEADLINE;
        roundtrip(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            &self.readable,
            deadline,
        )
        .await?;
        if self.state.finished {
            return Err(upstream("niri ended the data-control device"));
        }
        Ok(())
    }
}

/// Writes `bytes` to a reader's `fd` within `deadline`, then closes it.
pub(crate) async fn write(fd: OwnedFd, bytes: &[u8], deadline: Duration) -> Result<(), ToolError> {
    let mut writer = pipe::Sender::from_owned_fd(fd)
        .map_err(|error| upstream(&format!("use the reader's pipe: {error}")))?;
    tokio::time::timeout(deadline, writer.write_all(bytes))
        .await
        .map_err(|_| {
            ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("the reader didn't take the data within {deadline:?}"),
            )
        })?
        .map_err(|error| upstream(&format!("write to the reader: {error}")))
}

impl State {
    /// Keeps the selection's offer and destroys the others, which are no use any more.
    fn select(&mut self, selected: Option<ZwlrDataControlOfferV1>) {
        for (offer, _) in &self.offers {
            if selected.as_ref() != Some(offer) {
                offer.destroy();
            }
        }
        self.offers
            .retain(|(offer, _)| selected.as_ref() == Some(offer));
        self.selection = selected.map_or(Current::Empty, Current::Offer);
        self.announcements += 1;
    }
}

impl Synced for State {
    fn synced(&self) -> bool {
        self.synced
    }
    fn reset(&mut self) {
        self.synced = false;
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_device_v1::Event::DataOffer { id } = event {
            state.offers.push((id, Vec::new()));
        } else if let zwlr_data_control_device_v1::Event::Selection { id } = event {
            state.select(id);
        } else if matches!(event, zwlr_data_control_device_v1::Event::Finished) {
            state.finished = true;
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some((_, types)) = state.offers.iter_mut().find(|(known, _)| known == offer)
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, SourceId> for State {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        id: &SourceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_source_v1::Event::Send { mime_type, fd } = event {
            state.events.push_back(Event::Send {
                source: *id,
                mime: mime_type,
                fd,
            });
        } else if matches!(event, zwlr_data_control_source_v1::Event::Cancelled) {
            source.destroy();
            state.events.push_back(Event::Cancelled(*id));
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ZwlrDataControlManagerV1);

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use wayland_client::Proxy as _;

    use super::*;

    /// An offer for the state to hold. Nothing serves it: the state only counts it.
    fn offer() -> ZwlrDataControlOfferV1 {
        let (ours, _) = UnixStream::pair().unwrap();
        let connection = Connection::from_socket(ours).unwrap();
        ZwlrDataControlOfferV1::inert(connection.backend().downgrade())
    }

    /// A copy made while the save ran is an announcement after the saved one, whoever made
    /// it: the state can't tell our own source's offer from another client's, so a check
    /// that skipped any would let a newer copy be overwritten.
    #[test]
    fn every_announcement_after_the_save_makes_it_stale() {
        let mut state = State::default();
        state.select(Some(offer()));
        let saved = Saved {
            contents: None,
            announcement: state.announcements,
        };
        assert!(saved.still_current(state.announcements));
        state.select(None);
        assert!(
            !saved.still_current(state.announcements),
            "an empty selection"
        );
        let resaved = Saved {
            contents: None,
            announcement: state.announcements,
        };
        state.select(Some(offer()));
        assert!(!resaved.still_current(state.announcements), "an offer");
    }
}
