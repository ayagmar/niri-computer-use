//! The client's side of the bridge: its lines from stdin, and one writer for everything
//! that goes to its stdout. Neither may keep the relay from seeing that the client has
//! gone: the reader reports the client's end even while a line waits for the relay, and
//! the writer never makes the relay wait for the client to read.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;

use super::{Read, read_line};

/// How many lines may wait for the client to read them. A session runs at most 16 calls at
/// once, so a client that reads has no more than that waiting, with a few notifications
/// and local errors; a full queue means it has stopped reading. At worst the queue holds
/// this many engine lines of up to 256 MiB, 8 GiB, plus the one being written.
pub(crate) const QUEUE: usize = 32;
/// How long one line may take to reach the client, written and flushed, however slowly it
/// reads meanwhile.
pub(crate) const WRITE: Duration = Duration::from_secs(30);

/// How the client's input ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ended {
    /// It closed its end.
    Closed,
    /// A read failed, or a line was over the limit.
    Broken(String),
}

/// One of the client's reads, holding its room in the backlog until it is dropped.
pub(crate) type Waiting = (Read, OwnedSemaphorePermit);

/// Reads the client's lines, up to `max` bytes each, and passes them on, then how the input
/// ended. Lines totalling twice `max` may wait for the relay, so the reader usually reads
/// on to the end while the relay is stuck in a wait; and `ended` says so as soon as the
/// input ends, even while a line waits for room, so the relay can give up its wait. The
/// lines still come first in `lines`.
pub(crate) async fn pass_lines(
    input: impl AsyncRead + Unpin,
    max: usize,
    lines: mpsc::UnboundedSender<Waiting>,
    ended: watch::Sender<Option<Ended>>,
) {
    let backlog = Arc::new(Semaphore::new(max.saturating_mul(2)));
    let mut input = BufReader::new(input);
    loop {
        let read = read_line(&mut input, max).await;
        let size = match &read {
            Read::Line(line) => u32::try_from(line.len()).unwrap_or(u32::MAX),
            Read::End | Read::Broken(_) => 0,
        };
        let Some(room) = room(&backlog, size, &mut input, &ended).await else {
            return;
        };
        let end = ending(&read);
        if let Some(end) = &end {
            ended.send_replace(Some(end.clone()));
        }
        if lines.send((read, room)).is_err() || end.is_some() {
            return;
        }
    }
}

/// Room in `backlog` for `size` bytes, once there is, saying in `ended` if the input ends
/// meanwhile.
async fn room(
    backlog: &Arc<Semaphore>,
    size: u32,
    input: &mut BufReader<impl AsyncRead + Unpin>,
    ended: &watch::Sender<Option<Ended>>,
) -> Option<OwnedSemaphorePermit> {
    let acquire = Arc::clone(backlog).acquire_many_owned(size);
    tokio::pin!(acquire);
    let permit = tokio::select! {
        biased;
        permit = &mut acquire => permit,
        end = at_end(input) => {
            ended.send_replace(Some(end));
            acquire.await
        }
    };
    permit.ok()
}

/// How the input ended, when `read` is its end.
fn ending(read: &Read) -> Option<Ended> {
    match read {
        Read::Line(_) => None,
        Read::End => Some(Ended::Closed),
        Read::Broken(detail) => Some(Ended::Broken(detail.clone())),
    }
}

/// How the input ends, once it has: never, while more is buffered or on its way.
async fn at_end(input: &mut BufReader<impl AsyncRead + Unpin>) -> Ended {
    match input.fill_buf().await {
        Ok([]) => Ended::Closed,
        Ok(_) => std::future::pending().await,
        Err(error) => Ended::Broken(error.to_string()),
    }
}

/// The one writer of the client's stdout: a task writes the lines it is given, in order.
#[derive(Debug)]
pub(crate) struct Writer {
    queue: mpsc::Sender<Vec<u8>>,
    task: JoinHandle<Result<(), String>>,
}

impl Writer {
    /// Starts writing to `output`. The receiver says why writing failed, once it has.
    pub(crate) fn start(
        output: impl AsyncWrite + Unpin + Send + 'static,
    ) -> (Self, watch::Receiver<Option<String>>) {
        let (queue, lines) = mpsc::channel(QUEUE);
        let (failed, failure) = watch::channel(None);
        let task = tokio::spawn(write_lines(output, lines, failed));
        (Self { queue, task }, failure)
    }

    /// Queues `line`, without waiting: a full queue means the client has stopped reading.
    pub(crate) fn queue(&self, line: Vec<u8>) -> Result<(), String> {
        self.queue
            .try_send(line)
            .map_err(|_| format!("the client left {QUEUE} lines unread"))
    }

    /// Waits until every queued line is written, or writing failed.
    pub(crate) async fn finish(self) -> Result<(), String> {
        drop(self.queue);
        self.task
            .await
            .map_err(|error| format!("the writer to the client: {error}"))?
    }
}

/// Writes each line, with a newline if it lacks one, within `WRITE`, until the queue
/// closes or a write fails or takes too long.
async fn write_lines(
    mut output: impl AsyncWrite + Unpin,
    mut lines: mpsc::Receiver<Vec<u8>>,
    failed: watch::Sender<Option<String>>,
) -> Result<(), String> {
    while let Some(line) = lines.recv().await {
        let write = async {
            output.write_all(&line).await?;
            if !line.ends_with(b"\n") {
                output.write_all(b"\n").await?;
            }
            output.flush().await
        };
        let failure = match tokio::time::timeout(WRITE, write).await {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => format!("write to the client: {error}"),
            Err(_) => format!("the client didn't read a line within {} s", WRITE.as_secs()),
        };
        failed.send_replace(Some(failure.clone()));
        return Err(failure);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, duplex};

    use super::*;

    /// A relay stuck in a wait takes no lines: the client's end still shows, behind lines
    /// that fit the backlog and one that waits for room, and the lines still come first.
    #[tokio::test]
    async fn the_clients_end_shows_while_its_lines_wait() {
        let (mut client, input) = duplex(1024);
        let (lines, mut received) = mpsc::unbounded_channel();
        let (ended, mut end) = watch::channel(None);
        tokio::spawn(pass_lines(input, 100, lines, ended));
        let long = [vec![b'x'; 98], b"\n".to_vec()].concat();
        let sent = [b"a\n".to_vec(), b"b\n".to_vec(), long.clone(), long];
        client.write_all(&sent.concat()).await.unwrap();
        drop(client);
        let seen = tokio::time::timeout(Duration::from_secs(1), end.wait_for(Option::is_some))
            .await
            .expect("the end didn't show")
            .unwrap()
            .clone();
        assert_eq!(seen, Some(Ended::Closed));
        let mut order = Vec::new();
        while let Some((read, room)) = received.recv().await {
            drop(room);
            order.push(read);
        }
        let passed: Vec<_> = order
            .iter()
            .filter_map(|read| match read {
                Read::Line(line) => Some(line.clone()),
                Read::End | Read::Broken(_) => None,
            })
            .collect();
        assert_eq!(passed, sent);
        assert!(matches!(order.last(), Some(Read::End)), "{order:?}");
    }

    #[tokio::test]
    async fn lines_reach_the_client_in_order_each_ending_a_line() {
        let (output, mut client) = duplex(1024);
        let (writer, _) = Writer::start(output);
        writer.queue(b"a".to_vec()).unwrap();
        writer.queue(b"b\n".to_vec()).unwrap();
        writer.finish().await.unwrap();
        let mut written = Vec::new();
        client.read_to_end(&mut written).await.unwrap();
        assert_eq!(written, b"a\nb\n");
    }

    /// Reads a byte a second.
    async fn trickle(mut client: tokio::io::DuplexStream) {
        let mut byte = [0];
        while client.read_exact(&mut byte).await.is_ok() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// A client that reads a byte a second is still too slow: the deadline covers the
    /// whole line, not each step of it.
    #[tokio::test(start_paused = true)]
    async fn a_trickling_client_misses_the_deadline_of_the_line() {
        let (output, client) = duplex(1);
        tokio::spawn(trickle(client));
        let (writer, mut failure) = Writer::start(output);
        let started = tokio::time::Instant::now();
        writer.queue(vec![b'x'; 100]).unwrap();
        let why = failure.wait_for(Option::is_some).await.unwrap().clone();
        assert_eq!(started.elapsed(), WRITE);
        assert_eq!(
            why.as_deref(),
            Some("the client didn't read a line within 30 s")
        );
        assert_eq!(writer.finish().await, Err(why.unwrap()));
    }

    /// A client that reads nothing fills the queue, and the next line is refused at once.
    #[tokio::test]
    async fn a_full_queue_refuses_the_next_line_at_once() {
        let (output, _client) = duplex(1);
        let (writer, _) = Writer::start(output);
        let refused = (0..=QUEUE).find(|_| writer.queue(b"x\n".to_vec()).is_err());
        assert_eq!(refused, Some(QUEUE));
    }
}
