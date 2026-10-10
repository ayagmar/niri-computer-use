//! `harness slow-reader`: a clipboard reader in the nested session that is slow on
//! purpose, through ext-data-control. Once the selection offers the paste keeper's
//! password-manager hint, it asks for the text, writes `REQUESTED` in the test directory
//! once niri has passed the request on, waits `delay`, then reads to the end and writes
//! how many bytes arrived to `READ`.

use std::fs;
use std::io::Read as _;
use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    self, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{
    self, ExtDataControlOfferV1,
};

use crate::failure::{Context as _, Failure, Result};
use crate::nested::Nested;
use crate::test_dir::TestDir;
use crate::window::dispatch_until;

pub(crate) const USAGE: &str = "usage: harness slow-reader <TEST_DIR> <delay-ms> <deadline-ms>";
/// Written in the test directory once niri has the request for the text.
pub(crate) const REQUESTED: &str = "slow-reader-requested";
/// Written in the test directory with the number of bytes read.
pub(crate) const READ: &str = "slow-reader-read";
/// The type the paste keeper marks its text with.
const HINT: &str = "x-kde-passwordManagerHint";
const TEXT: &str = "text/plain;charset=utf-8";

#[derive(Debug, Default)]
struct State {
    /// Every offer niri introduced, with its types so far.
    offers: Vec<(ExtDataControlOfferV1, Vec<String>)>,
    /// The selection, once it offers `HINT`.
    keeper: Option<ExtDataControlOfferV1>,
}

pub(crate) fn run(args: &[&str]) -> Result<()> {
    let [test_dir, delay, deadline] = args else {
        return Err(Failure::new(USAGE));
    };
    let delay = Duration::from_millis(delay.parse().context(USAGE)?);
    let end = Instant::now() + Duration::from_millis(deadline.parse().context(USAGE)?);
    let test_dir = TestDir::open(PathBuf::from(test_dir))?;
    Nested::from_env(&test_dir)?;
    let connection = Connection::connect_to_env().context("connect to the nested niri")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).context("read the globals")?;
    let handle = queue.handle();
    let seat: WlSeat = globals.bind(&handle, 1..=1, ()).context("bind wl_seat")?;
    let manager: ExtDataControlManagerV1 = globals
        .bind(&handle, 1..=1, ())
        .context("bind ext_data_control_manager_v1")?;
    let _device = manager.get_data_device(&seat, &handle, ());
    let mut state = State::default();
    dispatch_until(&mut queue, &mut state, end, |state| state.keeper.is_some())?;
    let offer = state
        .keeper
        .clone()
        .ok_or_else(|| Failure::new("the selection never offered the keeper's hint"))?;
    let (mut reader, writer) = std::io::pipe().context("make a pipe")?;
    offer.receive(TEXT.to_owned(), writer.as_fd());
    // Once the round trip is back, niri has sent the keeper its `send`.
    queue.roundtrip(&mut state).context("ask for the text")?;
    drop(writer);
    let requested = test_dir.root().join(REQUESTED);
    fs::write(&requested, "").context(format!("write {}", requested.display()))?;
    pause(delay);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).context("read the text")?;
    let read = test_dir.root().join(READ);
    fs::write(&read, bytes.len().to_string()).context(format!("write {}", read.display()))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the harness is synchronous; the reader is slow on purpose, for a fixed delay"
)]
fn pause(duration: Duration) {
    std::thread::sleep(duration);
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_device_v1::Event::DataOffer { id } = event {
            state.offers.push((id, Vec::new()));
        } else if let ext_data_control_device_v1::Event::Selection { id: Some(id) } = event {
            let hinted = state
                .offers
                .iter()
                .any(|(offer, types)| *offer == id && types.iter().any(|mime| mime == HINT));
            if hinted {
                state.keeper = Some(id);
            }
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some((_, types)) = state.offers.iter_mut().find(|(known, _)| known == offer)
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ExtDataControlManagerV1);
