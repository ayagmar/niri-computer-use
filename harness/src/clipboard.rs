//! `harness clipboard`: a clipboard owner in the nested session, through ext-data-control
//! rather than the server's wlr protocol. It offers `TYPES`, each with contents of its
//! own, and with `--secret` also `SECRET_HINT` set to `secret`, as a password manager
//! marks what it copies. It serves reads until another client takes the selection, when
//! it writes `CANCELLED` in the test directory, or until its deadline. With `--hold`, a
//! slow owner, it answers no read of `HELD_TYPE` until another client takes the
//! selection: it writes `HELD` once it holds one, answers every held read once taken, and
//! writes no `CANCELLED`. Only a client that saves every type, as the paste keeper does,
//! reads that type; a clipboard manager that reads the text, as Noctalia's does, is
//! answered at once.

use std::fs::{self, File};
use std::io::Write as _;
use std::os::fd::OwnedFd;
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
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{
    self, ExtDataControlSourceV1,
};

use crate::failure::{Context as _, Failure, Result};
use crate::nested::Nested;
use crate::test_dir::TestDir;
use crate::window::dispatch_until;

pub(crate) const USAGE: &str =
    "usage: harness clipboard <TEST_DIR> <deadline-ms> [--secret | --hold]";
/// What the user copied: text, its markup and binary data, all different.
pub(crate) const TYPES: [(&str, &[u8]); 3] = [
    (
        "text/plain;charset=utf-8",
        "The user's own copy, café".as_bytes(),
    ),
    ("text/html", b"<b>The user's own copy</b>"),
    ("application/x-ncu-bytes", &[0, 1, 2, 0x7f, 0xfe, 0xff]),
];
/// The type password managers mark their copies with, and its value for a secret.
pub(crate) const SECRET_HINT: (&str, &[u8]) = ("x-kde-passwordManagerHint", b"secret");
/// Written in the test directory once another client took the selection.
pub(crate) const CANCELLED: &str = "clipboard-cancelled";
/// Written in the test directory once a `--hold` owner holds a read.
pub(crate) const HELD: &str = "clipboard-held";
/// The type whose reads a `--hold` owner holds: the binary one.
const HELD_TYPE: &str = TYPES[2].0;

#[derive(Debug, Default)]
struct State {
    /// Each type offered, with its bytes.
    offered: Vec<(&'static str, &'static [u8])>,
    /// Where a `--hold` owner notes that it holds a read.
    hold: Option<PathBuf>,
    /// The reads held, with what each asked for.
    held: Vec<(OwnedFd, &'static [u8])>,
    cancelled: bool,
    error: Option<Failure>,
}

pub(crate) fn run(args: &[&str]) -> Result<()> {
    let (test_dir, deadline, option) = match args {
        [test_dir, deadline] => (test_dir, deadline, None),
        [test_dir, deadline, option @ ("--secret" | "--hold")] => {
            (test_dir, deadline, Some(*option))
        }
        _ => return Err(Failure::new(USAGE)),
    };
    let test_dir = TestDir::open(PathBuf::from(test_dir))?;
    let mut state = State {
        offered: TYPES.to_vec(),
        ..State::default()
    };
    match option {
        Some("--secret") => state.offered.push(SECRET_HINT),
        Some(_) => state.hold = Some(test_dir.root().join(HELD)),
        None => {}
    }
    let end = Instant::now() + Duration::from_millis(deadline.parse().context(USAGE)?);
    Nested::from_env(&test_dir)?;
    let connection = Connection::connect_to_env().context("connect to the nested niri")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).context("read the globals")?;
    let handle = queue.handle();
    let seat: WlSeat = globals.bind(&handle, 1..=1, ()).context("bind wl_seat")?;
    let manager: ExtDataControlManagerV1 = globals
        .bind(&handle, 1..=1, ())
        .context("bind ext_data_control_manager_v1")?;
    let device = manager.get_data_device(&seat, &handle, ());
    let source = manager.create_data_source(&handle, ());
    for (mime, _) in &state.offered {
        source.offer((*mime).to_owned());
    }
    device.set_selection(Some(&source));
    dispatch_until(&mut queue, &mut state, end, |state| {
        state.cancelled || state.error.is_some()
    })?;
    for (fd, bytes) in state.held.drain(..) {
        File::from(fd)
            .write_all(bytes)
            .context("answer a held read")?;
    }
    if state.cancelled && state.hold.is_none() {
        let path = test_dir.root().join(CANCELLED);
        fs::write(&path, "").context(format!("write {}", path.display()))?;
    }
    state.error.map_or(Ok(()), Err)
}

impl State {
    /// Answers a read of `bytes` now, or holds it.
    fn answer(&mut self, fd: OwnedFd, mime: &str, bytes: &'static [u8]) {
        let Some(note) = self.hold.as_ref().filter(|_| mime == HELD_TYPE) else {
            if let Err(error) = File::from(fd).write_all(bytes) {
                self.error = Some(Failure::new(format!("send {mime}: {error}")));
            }
            return;
        };
        if self.held.is_empty()
            && let Err(error) = fs::write(note, "")
        {
            self.error = Some(Failure::new(format!("write {}: {error}", note.display())));
        }
        self.held.push((fd, bytes));
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_source_v1::Event::Send { mime_type, fd } = event {
            let Some(&(_, bytes)) = state.offered.iter().find(|(mime, _)| *mime == mime_type)
            else {
                state.error = Some(Failure::new(format!("asked for unoffered {mime_type}")));
                return;
            };
            state.answer(fd, &mime_type, bytes);
        } else if matches!(event, ext_data_control_source_v1::Event::Cancelled) {
            state.cancelled = true;
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlDeviceV1,
        _: ext_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
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
delegate_noop!(State: ignore ExtDataControlOfferV1);
