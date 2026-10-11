//! A fake clipboard owner on the fixture's Wayland display: just enough of the Wayland wire
//! protocol and of wlr data-control for the server to bind, see one selection and read its
//! types. The selection is a list of MIME types with their bytes, or none for nothing
//! copied, the same for every connection.

use std::io::{IoSliceMut, Write as _};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use tokio::io::{AsyncWriteExt as _, Interest};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::fixture::Fixture;

/// Each type with its bytes, in the order offered.
pub(crate) type Contents = Vec<(&'static str, Vec<u8>)>;

/// The type password managers mark their copies with.
pub(crate) const HINT: &str = "x-kde-passwordManagerHint";
/// The ids the fake gives the objects it creates: niri's first server-side id.
const OFFER: u32 = 0xff00_0000;
/// The registry names of the two globals the server binds.
const SEAT: u32 = 1;
const MANAGER: u32 = 2;

#[derive(Debug)]
struct Config {
    selection: Option<Contents>,
    /// A type whose reads are held unanswered until the reader goes away.
    hold: Option<&'static str>,
}

#[derive(Debug)]
pub(crate) struct Owner {
    config: Arc<Mutex<Config>>,
    /// One message per read held.
    held: mpsc::UnboundedReceiver<()>,
    /// One message per connection the server closed.
    closed: mpsc::UnboundedReceiver<()>,
    /// The types each read asked for, in order.
    reads: mpsc::UnboundedReceiver<String>,
}

/// The fake's ends of the channels to the test.
#[derive(Debug, Clone)]
struct Channels {
    config: Arc<Mutex<Config>>,
    held: mpsc::UnboundedSender<()>,
    closed: mpsc::UnboundedSender<()>,
    reads: mpsc::UnboundedSender<String>,
}

impl Owner {
    /// Serves `selection` on the fixture's display.
    pub(crate) fn serve(fixture: &Fixture, selection: Option<Contents>) -> Self {
        Self::on(fixture.display(), selection)
    }

    /// Serves `selection` on `listener`.
    pub(crate) fn on(
        listener: std::os::unix::net::UnixListener,
        selection: Option<Contents>,
    ) -> Self {
        listener.set_nonblocking(true).unwrap();
        let listener = UnixListener::from_std(listener).unwrap();
        let config = Arc::new(Mutex::new(Config {
            selection,
            hold: None,
        }));
        let (held, held_rx) = mpsc::unbounded_channel();
        let (closed, closed_rx) = mpsc::unbounded_channel();
        let (reads, reads_rx) = mpsc::unbounded_channel();
        let channels = Channels {
            config: Arc::clone(&config),
            held,
            closed,
            reads,
        };
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, channels.clone()));
            }
        });
        Self {
            config,
            held: held_rx,
            closed: closed_rx,
            reads: reads_rx,
        }
    }

    /// Makes `selection` what later connections see, as a copy would.
    pub(crate) fn copy(&self, selection: Option<Contents>) {
        self.config.lock().unwrap().selection = selection;
    }

    /// Holds every later read of `mime` unanswered until the reader goes away.
    pub(crate) fn hold(&self, mime: &'static str) {
        self.config.lock().unwrap().hold = Some(mime);
    }

    /// Waits until a read is held.
    pub(crate) async fn held(&mut self) {
        tokio::time::timeout(crate::client::WAIT, self.held.recv())
            .await
            .expect("no read was held")
            .unwrap();
    }

    /// Whether the server closed a connection within `limit`.
    pub(crate) async fn closed_within(&mut self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.closed.recv())
            .await
            .is_ok()
    }

    /// The types read so far, in order.
    pub(crate) fn reads(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.reads.try_recv().ok()).collect()
    }
}

/// What each object the server made is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Object {
    Display,
    Registry,
    Manager,
    Offer,
    Other,
}

/// One connection: reads requests, with the file descriptors sent alongside, and answers
/// them until the server closes it.
async fn serve(mut stream: UnixStream, channels: Channels) {
    let mut objects = vec![(1, Object::Display)];
    let mut bytes = Vec::new();
    let mut fds = Vec::new();
    let mut held = Vec::new();
    loop {
        let Some(read) = receive(&stream, &mut fds).await else {
            break;
        };
        bytes.extend(read);
        while let Some((message, rest)) = split(&bytes) {
            let events = handle(&message, &mut objects, &mut fds, &channels, &mut held);
            bytes = rest;
            if stream.write_all(&events).await.is_err() {
                break;
            }
        }
    }
    drop(held);
    channels.closed.send(()).ok();
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

/// Answers one request, and returns the events it causes.
fn handle(
    message: &[u8],
    objects: &mut Vec<(u32, Object)>,
    fds: &mut Vec<OwnedFd>,
    channels: &Channels,
    held: &mut Vec<OwnedFd>,
) -> Vec<u8> {
    let id = word(message, 0).unwrap();
    let opcode = word(message, 4).unwrap() & 0xffff;
    let mut args = Args {
        bytes: message,
        at: 8,
    };
    let object = objects
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
            objects.push((registry, Object::Registry));
            for (name, interface) in [(SEAT, "wl_seat"), (MANAGER, "zwlr_data_control_manager_v1")]
            {
                events.extend(event(
                    registry,
                    0,
                    &[Arg::Uint(name), Arg::Str(interface), Arg::Uint(1)],
                ));
            }
        }
        // wl_registry.bind
        (Object::Registry, 0) => {
            let name = args.uint();
            args.string();
            args.uint();
            let bound = if name == MANAGER {
                Object::Manager
            } else {
                Object::Other
            };
            objects.push((args.uint(), bound));
        }
        // zwlr_data_control_manager_v1.get_data_device
        (Object::Manager, 1) => {
            let device = args.uint();
            let selection = channels.config.lock().unwrap().selection.clone();
            events.extend(announce(device, selection.as_ref()));
            objects.push((OFFER, Object::Offer));
        }
        // zwlr_data_control_offer_v1.receive
        (Object::Offer, 0) => {
            let mime = args.string();
            let fd = fds.remove(0);
            answer(&mime, fd, channels, held);
        }
        _ => {}
    }
    events
}

/// The events that make the selection known on a new data device.
fn announce(device: u32, selection: Option<&Contents>) -> Vec<u8> {
    let Some(contents) = selection else {
        return event(device, 1, &[Arg::Uint(0)]);
    };
    let mut events = event(device, 0, &[Arg::Uint(OFFER)]);
    for (mime, _) in contents {
        events.extend(event(OFFER, 0, &[Arg::Str(mime)]));
    }
    events.extend(event(device, 1, &[Arg::Uint(OFFER)]));
    events
}

/// Writes what the selection holds as `mime` to the reader's `fd`, or holds the read.
fn answer(mime: &str, fd: OwnedFd, channels: &Channels, held: &mut Vec<OwnedFd>) {
    channels.reads.send(mime.to_owned()).ok();
    let (held_type, selection) = {
        let config = channels.config.lock().unwrap();
        (config.hold, config.selection.clone())
    };
    if held_type == Some(mime) {
        held.push(fd);
        channels.held.send(()).ok();
        return;
    }
    let bytes = selection
        .iter()
        .flatten()
        .find(|(offered, _)| *offered == mime)
        .map(|(_, bytes)| bytes.clone())
        .unwrap_or_default();
    // A reader may stop early, as at its cap: the write then fails, as the owner's would.
    tokio::task::spawn_blocking(move || std::fs::File::from(fd).write_all(&bytes).ok());
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
