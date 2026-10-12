//! A fake clipboard owner on a Wayland display, the fixture's or another: just enough of the Wayland wire
//! protocol and of wlr data-control for the server to bind, see one selection and read its
//! types. The selection is a list of MIME types with their bytes, or none for nothing
//! copied, the same for every connection, and a copy announces a new one on each. The
//! display also offers output `DP-1` and wlr virtual pointers, and hands the test every
//! motion and button a pointer sends. Each connection's registry request passes the hold
//! point `wayland:get_registry`.

use std::io::{IoSliceMut, Write as _};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use tokio::io::{AsyncWriteExt as _, Interest};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::client::Server;
use crate::fixture::Fixture;
use crate::holds::Points;
use crate::niri::{self, Niri};
use crate::noctalia::{self, UNLOCKED};

/// Each type with its bytes, in the order offered.
pub(crate) type Contents = Vec<(&'static str, Vec<u8>)>;

/// The type password managers mark their copies with.
pub(crate) const HINT: &str = "x-kde-passwordManagerHint";
/// The id the fake gives the first offer it creates, niri's first server-side id; later
/// offers count up from it.
const OFFER: u32 = 0xff00_0000;
/// The registry names of the two globals the server binds.
const SEAT: u32 = 1;
const MANAGER: u32 = 2;
const MANAGER_INTERFACE: &str = "zwlr_data_control_manager_v1";
/// The registry names of the output and the virtual pointer manager the pointer binds.
const OUTPUT: u32 = 3;
const POINTERS: u32 = 4;
/// The output's name, the fake niri's output.
const OUTPUT_NAME: &str = "DP-1";

/// What a virtual pointer sent: an absolute motion, as `(x, y, x_extent, y_extent)`, or a
/// button, as `(button, state)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pointed {
    Motion(u32, u32, u32, u32),
    Button(u32, u32),
}

#[derive(Debug)]
pub(crate) struct Owner {
    /// The selection: what a copy sets, announced to every data device at once.
    selection: watch::Sender<Option<Contents>>,
    /// A type whose reads are held unanswered until the reader goes away.
    hold: Arc<Mutex<Option<&'static str>>>,
    /// One message per read held.
    held: mpsc::UnboundedReceiver<()>,
    /// One message per connection with a data device that the server closed.
    closed: mpsc::UnboundedReceiver<()>,
    /// The types each read asked for, in order.
    reads: mpsc::UnboundedReceiver<String>,
    /// What every virtual pointer sent, in order.
    pointed: mpsc::UnboundedReceiver<Pointed>,
}

/// The fake's ends of the channels to the test.
#[derive(Debug, Clone)]
struct Channels {
    selection: watch::Receiver<Option<Contents>>,
    hold: Arc<Mutex<Option<&'static str>>>,
    held: mpsc::UnboundedSender<()>,
    closed: mpsc::UnboundedSender<()>,
    reads: mpsc::UnboundedSender<String>,
    pointed: mpsc::UnboundedSender<Pointed>,
    points: Points,
}

impl Owner {
    /// Serves `selection` on `listener`, passing `points`.
    pub(crate) fn on(
        listener: std::os::unix::net::UnixListener,
        selection: Option<Contents>,
        points: Points,
    ) -> Self {
        listener.set_nonblocking(true).unwrap();
        let listener = UnixListener::from_std(listener).unwrap();
        let (selection, selected) = watch::channel(selection);
        let held_type = Arc::new(Mutex::new(None));
        let (held, held_rx) = mpsc::unbounded_channel();
        let (closed, closed_rx) = mpsc::unbounded_channel();
        let (reads, reads_rx) = mpsc::unbounded_channel();
        let (pointed, pointed_rx) = mpsc::unbounded_channel();
        let channels = Channels {
            selection: selected,
            hold: Arc::clone(&held_type),
            held,
            closed,
            reads,
            pointed,
            points,
        };
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, channels.clone()));
            }
        });
        Self {
            selection,
            hold: held_type,
            held: held_rx,
            closed: closed_rx,
            reads: reads_rx,
            pointed: pointed_rx,
        }
    }

    /// Makes `selection` the selection, as a copy does, and announces it on every data
    /// device, before the device's connection answers anything sent after this.
    pub(crate) fn copy(&self, selection: Option<Contents>) {
        self.selection.send_replace(selection);
    }

    /// Holds every later read of `mime` unanswered until the reader goes away.
    pub(crate) fn hold(&self, mime: &'static str) {
        *self.hold.lock().unwrap() = Some(mime);
    }

    /// Waits until a read is held.
    pub(crate) async fn held(&mut self) {
        tokio::time::timeout(crate::client::WAIT, self.held.recv())
            .await
            .expect("no read was held")
            .unwrap();
    }

    /// Whether the server closed a connection with a data device within `limit`.
    pub(crate) async fn closed_within(&mut self, limit: std::time::Duration) -> bool {
        tokio::time::timeout(limit, self.closed.recv())
            .await
            .is_ok()
    }

    /// The types read so far, in order.
    pub(crate) fn reads(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.reads.try_recv().ok()).collect()
    }

    /// What virtual pointers sent since the last call, in order.
    pub(crate) fn pointed(&mut self) -> Vec<Pointed> {
        std::iter::from_fn(|| self.pointed.try_recv().ok()).collect()
    }
}

/// A server holding the lease, with `selection` copied before it took it.
#[derive(Debug)]
pub(crate) struct Leased {
    pub(crate) server: Server,
    _stream: niri::Stream,
    _niri: Niri,
    _noctalia: noctalia::Reply,
    /// Dropped last: every server must have exited first.
    pub(crate) fixture: Fixture,
}

impl Leased {
    pub(crate) async fn start(name: &str, selection: Option<Contents>) -> Self {
        let fixture = Fixture::new(name);
        fixture.program("noctalia", "exit 0");
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let mut niri = Niri::start(&fixture);
        fixture.clipboard.copy(selection);
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&[niri::window_on(1, Some("a"), 1, true)]);
        server.structured("acquire_desktop").await;
        Self {
            fixture,
            server,
            _niri: niri,
            _stream: stream,
            _noctalia: noctalia,
        }
    }

    pub(crate) const fn owner(&mut self) -> &mut Owner {
        &mut self.fixture.clipboard
    }

    pub(crate) async fn read(&mut self) -> serde_json::Value {
        self.server.structured("clipboard_read").await
    }
}

/// What each object the server made is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Object {
    Display,
    Registry,
    Manager,
    Output,
    Pointers,
    Pointer,
    /// The offer with this index among the connection's.
    Offer(usize),
    Other,
}

/// One connection's objects, offers and the descriptors it was sent.
#[derive(Debug)]
struct Connection {
    objects: Vec<(u32, Object)>,
    /// Each offer announced, with its contents.
    offers: Vec<Contents>,
    device: Option<u32>,
    fds: Vec<OwnedFd>,
    /// Reads held, closed when the connection ends.
    held: Vec<OwnedFd>,
    channels: Channels,
}

/// One connection: reads requests, with the file descriptors sent alongside, answers them
/// and announces each copy, until the server closes it.
async fn serve(mut stream: UnixStream, channels: Channels) {
    let mut copies = channels.selection.clone();
    let mut connection = Connection {
        objects: vec![(1, Object::Display)],
        offers: Vec::new(),
        device: None,
        fds: Vec::new(),
        held: Vec::new(),
        channels,
    };
    let mut bytes = Vec::new();
    loop {
        let events = tokio::select! {
            read = receive(&stream, &mut connection.fds) => {
                let Some(read) = read else { break };
                bytes.extend(read);
                // A copy made before the request was sent is announced before its answer.
                let mut events = if copies.has_changed().unwrap_or(false) {
                    let selection = copies.borrow_and_update().clone();
                    connection.announce(selection)
                } else {
                    Vec::new()
                };
                while let Some((message, rest)) = split(&bytes) {
                    if let Some(point) = point(&message) {
                        drop(connection.channels.points.pass(point.to_owned()).await);
                    }
                    events.extend(connection.handle(&message));
                    bytes = rest;
                }
                events
            }
            Ok(()) = copies.changed() => {
                let selection = copies.borrow_and_update().clone();
                connection.announce(selection)
            }
        };
        if stream.write_all(&events).await.is_err() {
            break;
        }
    }
    if connection.device.is_some() {
        connection.channels.closed.send(()).ok();
    }
}

/// The next bytes, with any file descriptors appended to `fds`; `None` at the end.
async fn receive(stream: &UnixStream, fds: &mut Vec<OwnedFd>) -> Option<Vec<u8>> {
    let mut buffer = vec![0; 4096];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(8))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let received = stream
        .async_io(Interest::READABLE, || {
            rustix::net::recvmsg(
                stream,
                &mut [IoSliceMut::new(&mut buffer)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC,
            )
            .map_err(std::io::Error::from)
        })
        .await
        .ok()?;
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(passed) = message {
            fds.extend(passed);
        }
    }
    if received.bytes == 0 {
        return None;
    }
    buffer.truncate(received.bytes);
    Some(buffer)
}

/// The hold point a request passes before it is answered, if any. The server waits for
/// the registry's answer, so a held one holds the server.
fn point(message: &[u8]) -> Option<&'static str> {
    let id = word(message, 0)?;
    let opcode = word(message, 4)? & 0xffff;
    (id == 1 && opcode == 1).then_some("wayland:get_registry")
}

/// A whole message off the front of `bytes`, and what follows it.
fn split(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let size = (word(bytes, 4)? >> 16) as usize;
    (size >= 8 && bytes.len() >= size).then(|| (bytes[..size].to_vec(), bytes[size..].to_vec()))
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A request's arguments, read in order.
struct Args<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Args<'_> {
    fn uint(&mut self) -> u32 {
        let value = word(self.bytes, self.at).unwrap();
        self.at += 4;
        value
    }

    fn string(&mut self) -> String {
        let length = self.uint() as usize;
        let bytes = &self.bytes[self.at..self.at + length - 1];
        self.at += length.next_multiple_of(4);
        String::from_utf8(bytes.to_vec()).unwrap()
    }
}

impl Connection {
    /// Answers one request, and returns the events it causes.
    fn handle(&mut self, message: &[u8]) -> Vec<u8> {
        let id = word(message, 0).unwrap();
        let opcode = word(message, 4).unwrap() & 0xffff;
        let mut args = Args {
            bytes: message,
            at: 8,
        };
        let object = self
            .objects
            .iter()
            .find(|(known, _)| *known == id)
            .map_or(Object::Other, |(_, object)| *object);
        let mut events = Vec::new();
        match (object, opcode) {
            // wl_display.sync
            (Object::Display, 0) => {
                let callback = args.uint();
                events.extend(event(callback, 0, &[Arg::Uint(0)]));
                events.extend(event(1, 1, &[Arg::Uint(callback)]));
            }
            // wl_display.get_registry
            (Object::Display, 1) => {
                let registry = args.uint();
                self.objects.push((registry, Object::Registry));
                for (name, interface, version) in [
                    (SEAT, "wl_seat", 1),
                    (MANAGER, MANAGER_INTERFACE, 1),
                    (OUTPUT, "wl_output", 4),
                    (POINTERS, "zwlr_virtual_pointer_manager_v1", 2),
                ] {
                    events.extend(event(
                        registry,
                        0,
                        &[Arg::Uint(name), Arg::Str(interface), Arg::Uint(version)],
                    ));
                }
            }
            // wl_registry.bind
            (Object::Registry, 0) => {
                let name = args.uint();
                args.string();
                args.uint();
                let bound = match name {
                    MANAGER => Object::Manager,
                    OUTPUT => Object::Output,
                    POINTERS => Object::Pointers,
                    _ => Object::Other,
                };
                let new = args.uint();
                self.objects.push((new, bound));
                if bound == Object::Output {
                    // wl_output.name, then wl_output.done
                    events.extend(event(new, 4, &[Arg::Str(OUTPUT_NAME)]));
                    events.extend(event(new, 2, &[]));
                }
            }
            // zwlr_virtual_pointer_manager_v1.create_virtual_pointer_with_output
            (Object::Pointers, 2) => {
                args.uint();
                args.uint();
                self.objects.push((args.uint(), Object::Pointer));
            }
            // zwlr_virtual_pointer_v1.motion_absolute
            (Object::Pointer, 1) => {
                args.uint();
                let (x, y) = (args.uint(), args.uint());
                let motion = Pointed::Motion(x, y, args.uint(), args.uint());
                self.channels.pointed.send(motion).ok();
            }
            // zwlr_virtual_pointer_v1.button
            (Object::Pointer, 2) => {
                args.uint();
                let button = Pointed::Button(args.uint(), args.uint());
                self.channels.pointed.send(button).ok();
            }
            // zwlr_data_control_manager_v1.get_data_device
            (Object::Manager, 1) => {
                self.device = Some(args.uint());
                let selection = self.channels.selection.borrow().clone();
                events.extend(self.announce(selection));
            }
            // zwlr_data_control_offer_v1.receive
            (Object::Offer(index), 0) => {
                let mime = args.string();
                let fd = self.fds.remove(0);
                self.answer(index, &mime, fd);
            }
            _ => {}
        }
        events
    }

    /// The events that make `selection` known on the data device, if there is one yet.
    fn announce(&mut self, selection: Option<Contents>) -> Vec<u8> {
        let Some(device) = self.device else {
            return Vec::new();
        };
        let Some(contents) = selection else {
            return event(device, 1, &[Arg::Uint(0)]);
        };
        let index = self.offers.len();
        let offer = OFFER + u32::try_from(index).unwrap();
        let mut events = event(device, 0, &[Arg::Uint(offer)]);
        for (mime, _) in &contents {
            events.extend(event(offer, 0, &[Arg::Str(mime)]));
        }
        events.extend(event(device, 1, &[Arg::Uint(offer)]));
        self.objects.push((offer, Object::Offer(index)));
        self.offers.push(contents);
        events
    }

    /// Writes what offer `index` holds as `mime` to the reader's `fd`, or holds the read.
    fn answer(&mut self, index: usize, mime: &str, fd: OwnedFd) {
        self.channels.reads.send(mime.to_owned()).ok();
        if *self.channels.hold.lock().unwrap() == Some(mime) {
            self.held.push(fd);
            self.channels.held.send(()).ok();
            return;
        }
        let bytes = self.offers[index]
            .iter()
            .find(|(offered, _)| *offered == mime)
            .map(|(_, bytes)| bytes.clone())
            .unwrap_or_default();
        // A reader may stop early, as at its cap: the write then fails, as the owner's
        // would.
        tokio::task::spawn_blocking(move || std::fs::File::from(fd).write_all(&bytes).ok());
    }
}

enum Arg<'a> {
    Uint(u32),
    Str(&'a str),
}

fn event(object: u32, opcode: u16, args: &[Arg<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for arg in args {
        match arg {
            Arg::Uint(value) => body.extend(value.to_ne_bytes()),
            Arg::Str(text) => {
                let length = u32::try_from(text.len() + 1).unwrap();
                body.extend(length.to_ne_bytes());
                body.extend(text.as_bytes());
                body.resize(
                    body.len() + (length as usize).next_multiple_of(4) - text.len(),
                    0,
                );
            }
        }
    }
    let size = u32::try_from(body.len() + 8).unwrap();
    let mut message = object.to_ne_bytes().to_vec();
    message.extend(((size << 16) | u32::from(opcode)).to_ne_bytes());
    message.extend(body);
    message
}
