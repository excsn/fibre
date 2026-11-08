//! Waiting on a socket: `poll(2)` for a blocking caller, tokio's reactor for an async one.
//!
//! The socket stays non-blocking for its whole life. It is registered with tokio once, on the first async wait, and registered again when that registration's runtime has gone.

use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio::io::Interest as TokioInterest;
use tokio::io::unix::AsyncFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interest {
  pub read: bool,
  pub write: bool,
}

#[cfg_attr(not(feature = "rustls"), allow(dead_code))]
impl Interest {
  pub const READ: Interest = Interest { read: true, write: false };
  pub const WRITE: Interest = Interest { read: false, write: true };
}

/// Waits until the socket is ready or the deadline passes. Returns `false` at the deadline.
pub(crate) fn block(fd: RawFd, interest: Interest, deadline: Option<Instant>) -> io::Result<bool> {
  let mut events = 0;
  if interest.read {
    events |= libc::POLLIN;
  }
  if interest.write {
    events |= libc::POLLOUT;
  }
  loop {
    let timeout = match deadline {
      None => -1,
      Some(d) => {
        let now = Instant::now();
        if now >= d {
          return Ok(false);
        }
        (d - now).as_millis().clamp(1, i32::MAX as u128) as i32
      }
    };
    let mut pfd = libc::pollfd { fd, events, revents: 0 };
    let r = unsafe { libc::poll(&mut pfd, 1, timeout) };
    if r < 0 {
      let e = io::Error::last_os_error();
      if e.kind() == io::ErrorKind::Interrupted {
        continue;
      }
      return Err(e);
    }
    if r > 0 {
      return Ok(true);
    }
  }
}

#[derive(Debug)]
struct Fd(RawFd);

impl AsRawFd for Fd {
  fn as_raw_fd(&self) -> RawFd {
    self.0
  }
}

/// The socket's registration with a tokio runtime.
#[derive(Default)]
pub(crate) struct Registration(Mutex<Option<Arc<AsyncFd<Fd>>>>);

impl Registration {
  fn get(&self, fd: RawFd) -> io::Result<Arc<AsyncFd<Fd>>> {
    let mut slot = self.0.lock();
    if let Some(afd) = slot.as_ref() {
      return Ok(afd.clone());
    }
    // SAFETY: the connection owns the socket and drops this registration before it closes the socket.
    let afd = unsafe { AsyncFd::register_with_interest(Fd(fd), TokioInterest::READABLE | TokioInterest::WRITABLE) };
    let afd = Arc::new(afd.map_err(|e| io::Error::other(format!("{e:?}")))?);
    *slot = Some(afd.clone());
    Ok(afd)
  }

  fn forget(&self) {
    self.0.lock().take();
  }

  /// Waits until the socket is ready or the deadline passes. Returns `false` at the deadline.
  pub(crate) async fn wait(&self, fd: RawFd, interest: Interest, deadline: Option<Instant>) -> io::Result<bool> {
    let wanted = match (interest.read, interest.write) {
      (true, true) => TokioInterest::READABLE | TokioInterest::WRITABLE,
      (false, true) => TokioInterest::WRITABLE,
      _ => TokioInterest::READABLE,
    };
    for _ in 0..2 {
      let afd = self.get(fd)?;
      let ready = async {
        let mut guard = afd.ready(wanted).await?;
        guard.clear_ready();
        Ok::<(), io::Error>(())
      };
      let result = match deadline {
        Some(d) => match tokio::time::timeout_at(d.into(), ready).await {
          Ok(r) => r,
          Err(_) => return Ok(false),
        },
        None => ready.await,
      };
      match result {
        Ok(()) => return Ok(true),
        Err(_) => self.forget(),
      }
    }
    Err(io::Error::other("socket could not be registered with the current tokio runtime"))
  }
}
