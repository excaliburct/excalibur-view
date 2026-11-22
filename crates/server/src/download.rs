//! The update download for a computer nobody has signed in on (review M2):
//! a few at a time, a few a minute from one address, and sent from the disk
//! a piece at a time rather than read whole into memory.

use std::collections::HashMap; // WHAT: a map of addresses to their count. WHY: the per-address limit
use std::pin::Pin; // WHAT: a pinned reference. WHY: the body's poll takes one
use std::sync::{Arc, Mutex}; // WHAT: shared limits and the lock around the counts. WHY: one list for all requests
use std::task::{Context, Poll}; // WHAT: the async poll's types. WHY: the body is polled by the connection
use std::time::{Duration, Instant}; // WHAT: spans and moments. WHY: the rate window

use hyper::body::{Body, Bytes, Frame, SizeHint}; // WHAT: the body trait and its pieces. WHY: a body that reads the file as it is sent, with no new crate
use tokio::io::{AsyncRead, ReadBuf}; // WHAT: reading a file in pieces. WHY: only a piece is in memory at once
use tokio::sync::{OwnedSemaphorePermit, Semaphore}; // WHAT: a counted limit whose place is held by its owner. WHY: the place is held until the last byte is sent

/// How many downloads may run at once for computers not signed in.
pub const AT_ONCE: usize = 4; // WHAT: four. WHY: a stranger on the office network can't hold the server's disk and sockets; the office's seats update a few at a time and retry

/// How many downloads one address may start in a window.
pub const PER_WINDOW: u32 = 10; // WHAT: ten. WHY: one computer fetches a build once; ten is room for retries and a few computers behind one address

/// The window the per-address count is kept over.
pub const WINDOW: Duration = Duration::from_secs(5 * 60); // WHAT: five minutes. WHY: long enough to stop a loop, short enough that an office is never kept waiting

/// The most addresses remembered at once.
const MOST_KEPT: usize = 4096; // WHAT: 4096 addresses. WHY: a stranger with many addresses can't grow the list without end

/// The limits on the anonymous download, one set to a server.
pub struct Limits {
    places: Arc<Semaphore>, // WHAT: the downloads' places. WHY: taken before a file is opened, given back when the last byte is sent
    seen: Mutex<HashMap<String, (Instant, u32)>>, // WHAT: each address's window start and count. WHY: the per-address limit
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            places: Arc::new(Semaphore::new(AT_ONCE)),
            seen: Mutex::default(),
        } // WHAT: all places free, nobody counted. WHY: empty on every start, as patience is
    }
}

/// Why an anonymous download was turned away.
#[derive(Debug, PartialEq, Eq)]
pub enum Turned {
    TooOften, // WHAT: this address has started too many. WHY: told to wait a while
    Busy,     // WHAT: all the places are in use. WHY: told to retry shortly
}

impl Limits {
    /// Counts one more download from `from` and takes a place for it.
    ///
    /// # Errors
    ///
    /// [`Turned::TooOften`] when the address has used up its window, and
    /// [`Turned::Busy`] when every place is taken.
    pub fn take(&self, from: &str) -> Result<OwnedSemaphorePermit, Turned> {
        if !from.is_empty() {
            let mut seen = self.seen.lock().map_err(|_| Turned::Busy)?; // WHAT: the counts. WHY: a poisoned lock fails closed
            let now = Instant::now(); // WHAT: this moment. WHY: compared with each window's start
            if seen.len() >= MOST_KEPT {
                seen.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
                // WHAT: finished windows forgotten. WHY: the list stays small
            }
            let entry = seen.entry(from.to_string()).or_insert((now, 0)); // WHAT: this address's window. WHY: counted below
            if now.duration_since(entry.0) >= WINDOW {
                *entry = (now, 0); // WHAT: a new window. WHY: the old one is over
            }
            if entry.1 >= PER_WINDOW {
                return Err(Turned::TooOften); // WHAT: refused. WHY: the address has had its share
            }
            entry.1 += 1; // WHAT: this one counted. WHY: even one that is then told to retry counts, so a loop gets no further
        }
        self.places
            .clone()
            .try_acquire_owned()
            .map_err(|_| Turned::Busy) // WHAT: a place, or told to retry. WHY: never queued, so waiting connections cost nothing
    }
}

/// A file sent as it is read, holding its place until it is done.
pub struct Streamed {
    file: tokio::fs::File, // WHAT: the open file. WHY: read a piece at a time
    chunk: Vec<u8>,        // WHAT: the piece's buffer. WHY: the only memory the file ever takes
    left: u64, // WHAT: how many bytes are still to send. WHY: the size hint, and the end if the file grew
    _place: Option<OwnedSemaphorePermit>, // WHAT: the download's place, if it took one. WHY: given back when the body is dropped, finished or not
}

impl Streamed {
    /// Opens `path` to send, with the place it holds, if any.
    ///
    /// # Errors
    ///
    /// When the file can't be opened or measured.
    pub async fn open(
        path: &std::path::Path,
        place: Option<OwnedSemaphorePermit>,
    ) -> std::io::Result<(Streamed, u64)> {
        let file = tokio::fs::File::open(path).await?; // WHAT: the file. WHY: opened here, so a missing one is an error before any answer is sent
        let size = file.metadata().await?.len(); // WHAT: its length. WHY: the Content-Length, and the count left
        Ok((
            Streamed {
                file,
                chunk: vec![0; 64 * 1024],
                left: size,
                _place: place,
            },
            size,
        )) // WHAT: the body, 64 KiB at a time. WHY: a few connections never hold a whole installer
    }
}

impl Body for Streamed {
    type Data = Bytes; // WHAT: bytes. WHY: what a frame carries
    type Error = std::io::Error; // WHAT: a read error. WHY: the connection is cut rather than a short file passed off as whole

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut(); // WHAT: the body, unpinned. WHY: every field is Unpin
        if this.left == 0 {
            return Poll::Ready(None); // WHAT: the end. WHY: everything promised was sent
        }
        let want = usize::try_from(this.left)
            .unwrap_or(usize::MAX)
            .min(this.chunk.len()); // WHAT: no more than what is left. WHY: never more than the Content-Length said
        let mut buf = ReadBuf::new(&mut this.chunk[..want]); // WHAT: the piece to fill. WHY: read into
        match Pin::new(&mut this.file).poll_read(cx, &mut buf) {
            Poll::Pending => Poll::Pending, // WHAT: not yet. WHY: woken when the disk answers
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))), // WHAT: the failure. WHY: the connection is cut
            Poll::Ready(Ok(())) => {
                let got = buf.filled(); // WHAT: what was read. WHY: sent as it is
                if got.is_empty() {
                    return Poll::Ready(Some(Err(std::io::ErrorKind::UnexpectedEof.into())));
                    // WHAT: the file ended early. WHY: cut, so no one installs half a build
                }
                this.left = this
                    .left
                    .saturating_sub(u64::try_from(got.len()).unwrap_or(u64::MAX)); // WHAT: counted off. WHY: the end is reached by it
                Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(got))))) // WHAT: the piece. WHY: sent on
            }
        }
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.left) // WHAT: exactly what is left. WHY: the connection can send a length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_so_many_places_and_one_is_given_back_when_a_download_ends() {
        let limits = Limits::default(); // WHAT: a fresh set. WHY: nothing counted
        let held: Vec<_> = (0..AT_ONCE)
            .map(|n| limits.take(&format!("10.0.0.{n}")).unwrap())
            .collect(); // WHAT: every place taken, each from its own address. WHY: so only the places run out
        assert_eq!(
            limits.take("10.0.1.1").err(),
            Some(Turned::Busy),
            "the next is told to retry"
        ); // WHAT: refused. WHY: the cap
        drop(held); // WHAT: the downloads end. WHY: their places come back
        assert!(limits.take("10.0.1.2").is_ok(), "a place is free again"); // WHAT: let in. WHY: a place is not lost for good
    }

    #[test]
    fn one_address_has_only_so_many_in_a_window_and_another_is_not_held_to_it() {
        let limits = Limits::default(); // WHAT: a fresh set. WHY: nothing counted
        for _ in 0..PER_WINDOW {
            drop(limits.take("10.0.0.9").unwrap()); // WHAT: a download started and ended. WHY: only the count is under test
        }
        assert_eq!(
            limits.take("10.0.0.9").err(),
            Some(Turned::TooOften),
            "past the share"
        ); // WHAT: refused. WHY: the per-address limit
        assert!(
            limits.take("10.0.0.10").is_ok(),
            "another address is not held to it"
        ); // WHAT: let in. WHY: one address never spends another's share
    }

    #[test]
    fn a_window_that_is_over_is_forgotten() {
        let limits = Limits::default(); // WHAT: a fresh set. WHY: nothing counted
        for _ in 0..PER_WINDOW {
            drop(limits.take("10.0.0.9").unwrap()); // WHAT: the share used up. WHY: to be forgotten
        }
        limits.seen.lock().unwrap().get_mut("10.0.0.9").unwrap().0 = Instant::now()
            .checked_sub(WINDOW + Duration::from_secs(1))
            .unwrap(); // WHAT: the window's start moved back past its end. WHY: as if the minutes had passed
        assert!(limits.take("10.0.0.9").is_ok(), "a new window"); // WHAT: let in. WHY: an office is never kept waiting for good
    }
}
