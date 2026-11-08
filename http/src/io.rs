//! Byte transports: a plain TCP socket or rustls over one, both non-blocking.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, RawFd};

use bytes::BytesMut;

use crate::wait::Interest;

pub(crate) enum Io {
  Plain(TcpStream),
  #[cfg(feature = "rustls")]
  Tls(Box<(TcpStream, rustls::ClientConnection)>),
  #[cfg(feature = "native-tls")]
  Native(Box<native::NativeIo>),
  #[cfg(shuttle)]
  Model(std::sync::Arc<model::Pipe>),
}

impl Io {
  pub(crate) fn fd(&self) -> RawFd {
    match self {
      Io::Plain(s) => s.as_raw_fd(),
      #[cfg(feature = "rustls")]
      Io::Tls(t) => t.0.as_raw_fd(),
      #[cfg(feature = "native-tls")]
      Io::Native(n) => n.sock.as_raw_fd(),
      #[cfg(shuttle)]
      Io::Model(_) => -1,
    }
  }

  pub(crate) fn waitable(&self) -> crate::wait::Waitable {
    match self {
      #[cfg(shuttle)]
      Io::Model(pipe) => crate::wait::Waitable::Model(pipe.clone()),
      _ => crate::wait::Waitable::Fd(self.fd()),
    }
  }

  /// Reads plaintext. `Ok(0)` is the end of the stream.
  pub(crate) fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    match self {
      Io::Plain(s) => s.read(buf),
      #[cfg(feature = "rustls")]
      Io::Tls(t) => {
        let (sock, tls) = &mut **t;
        loop {
          match tls.reader().read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
            Err(e) => return Err(e),
          }
          match tls.read_tls(sock) {
            Ok(0) => return Ok(0),
            Ok(_) => {
              tls.process_new_packets().map_err(io::Error::other)?;
            }
            Err(e) => return Err(e),
          }
        }
      }
      #[cfg(feature = "native-tls")]
      Io::Native(n) => n.read(buf),
      #[cfg(shuttle)]
      Io::Model(pipe) => pipe.read(buf),
    }
  }

  /// Writes as much of `pending` as the socket takes and removes it. Returns whether everything went out.
  pub(crate) fn flush(&mut self, pending: &mut BytesMut) -> io::Result<bool> {
    match self {
      Io::Plain(s) => {
        while !pending.is_empty() {
          match s.write(pending) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
              let _ = pending.split_to(n);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e),
          }
        }
        Ok(true)
      }
      #[cfg(feature = "rustls")]
      Io::Tls(t) => {
        let (sock, tls) = &mut **t;
        if !pending.is_empty() {
          tls.writer().write_all(pending)?;
          pending.clear();
        }
        while tls.wants_write() {
          match tls.write_tls(sock) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e),
          }
        }
        Ok(true)
      }
      #[cfg(feature = "native-tls")]
      Io::Native(n) => n.flush(pending),
      #[cfg(shuttle)]
      Io::Model(pipe) => {
        pipe.send(pending);
        pending.clear();
        Ok(true)
      }
    }
  }

  /// Moves the TLS handshake on. Returns what to wait for, or `None` once it is done.
  pub(crate) fn handshake(&mut self) -> io::Result<Option<Interest>> {
    match self {
      Io::Plain(_) => Ok(None),
      #[cfg(feature = "rustls")]
      Io::Tls(t) => {
        let (sock, tls) = &mut **t;
        loop {
          while tls.wants_write() {
            match tls.write_tls(sock) {
              Ok(_) => {}
              Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Some(Interest::WRITE)),
              Err(e) => return Err(e),
            }
          }
          if !tls.is_handshaking() {
            return Ok(None);
          }
          match tls.read_tls(sock) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) => {
              tls.process_new_packets().map_err(io::Error::other)?;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Some(Interest::READ)),
            Err(e) => return Err(e),
          }
        }
      }
      #[cfg(feature = "native-tls")]
      Io::Native(n) => n.handshake(),
      #[cfg(shuttle)]
      Io::Model(_) => Ok(None),
    }
  }

  pub(crate) fn alpn(&self) -> Option<Vec<u8>> {
    match self {
      Io::Plain(_) => None,
      #[cfg(feature = "rustls")]
      Io::Tls(t) => t.1.alpn_protocol().map(<[u8]>::to_vec),
      #[cfg(feature = "native-tls")]
      Io::Native(n) => n.alpn(),
      #[cfg(shuttle)]
      Io::Model(_) => None,
    }
  }

  /// Whether a non-blocking peek on an idle connection finds it closed or holding bytes nobody asked for.
  pub(crate) fn idle_unusable(&self) -> bool {
    let mut byte = 0u8;
    let n = unsafe { libc::recv(self.fd(), (&mut byte as *mut u8).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
    n >= 0
  }
}

#[cfg(feature = "native-tls")]
pub(crate) mod native {
  //! native-tls owns the stream it encrypts, so it gets two byte buffers and this moves ciphertext between them and the socket.

  use std::collections::VecDeque;
  use std::io::{self, Read, Write};
  use std::net::TcpStream;

  use bytes::BytesMut;
  use native_tls::{HandshakeError, MidHandshakeTlsStream, TlsConnector, TlsStream};

  use crate::wait::Interest;

  #[derive(Default)]
  pub(crate) struct Mem {
    inbound: VecDeque<u8>,
    outbound: Vec<u8>,
    eof: bool,
  }

  impl Read for Mem {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      if self.inbound.is_empty() {
        return if self.eof { Ok(0) } else { Err(io::ErrorKind::WouldBlock.into()) };
      }
      let n = buf.len().min(self.inbound.len());
      for (slot, byte) in buf.iter_mut().zip(self.inbound.drain(..n)) {
        *slot = byte;
      }
      Ok(n)
    }
  }

  impl Write for Mem {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      self.outbound.extend_from_slice(buf);
      Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }

  enum State {
    Start(TlsConnector, String),
    Mid(MidHandshakeTlsStream<Mem>),
    Ready(TlsStream<Mem>),
    Failed,
  }

  pub(crate) struct NativeIo {
    pub(crate) sock: TcpStream,
    state: State,
  }

  /// Writes `mem`'s ciphertext to the socket. Returns whether all of it went.
  fn write_out(sock: &mut TcpStream, mem: &mut Mem) -> io::Result<bool> {
    while !mem.outbound.is_empty() {
      match sock.write(&mem.outbound) {
        Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
        Ok(n) => {
          mem.outbound.drain(..n);
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
        Err(e) => return Err(e),
      }
    }
    Ok(true)
  }

  /// Reads ciphertext from the socket into `mem`. Returns whether anything arrived, the end included.
  fn read_in(sock: &mut TcpStream, mem: &mut Mem) -> io::Result<bool> {
    let mut buf = [0u8; 16 * 1024];
    match sock.read(&mut buf) {
      Ok(0) => {
        mem.eof = true;
        Ok(true)
      }
      Ok(n) => {
        mem.inbound.extend(&buf[..n]);
        Ok(true)
      }
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
      Err(e) => Err(e),
    }
  }

  fn failure(e: native_tls::Error) -> io::Error {
    io::Error::other(e.to_string())
  }

  impl NativeIo {
    pub(crate) fn new(sock: TcpStream, connector: TlsConnector, host: String) -> Self {
      Self { sock, state: State::Start(connector, host) }
    }

    pub(crate) fn handshake(&mut self) -> io::Result<Option<Interest>> {
      loop {
        self.state = match std::mem::replace(&mut self.state, State::Failed) {
          State::Start(connector, host) => match connector.connect(&host, Mem::default()) {
            Ok(tls) => State::Ready(tls),
            Err(HandshakeError::WouldBlock(mid)) => State::Mid(mid),
            Err(HandshakeError::Failure(e)) => return Err(failure(e)),
          },
          State::Mid(mut mid) => {
            if !write_out(&mut self.sock, mid.get_mut())? {
              self.state = State::Mid(mid);
              return Ok(Some(Interest::WRITE));
            }
            if !read_in(&mut self.sock, mid.get_mut())? {
              self.state = State::Mid(mid);
              return Ok(Some(Interest::READ));
            }
            match mid.handshake() {
              Ok(tls) => State::Ready(tls),
              Err(HandshakeError::WouldBlock(mid)) => State::Mid(mid),
              Err(HandshakeError::Failure(e)) => return Err(failure(e)),
            }
          }
          State::Ready(mut tls) => {
            let drained = write_out(&mut self.sock, tls.get_mut())?;
            self.state = State::Ready(tls);
            return Ok(if drained { None } else { Some(Interest::WRITE) });
          }
          State::Failed => return Err(io::Error::other("tls handshake failed")),
        };
      }
    }

    pub(crate) fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      let State::Ready(tls) = &mut self.state else { return Err(io::ErrorKind::NotConnected.into()) };
      loop {
        match tls.read(buf) {
          Ok(n) => return Ok(n),
          Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            write_out(&mut self.sock, tls.get_mut())?;
            if tls.get_ref().eof && tls.get_ref().inbound.is_empty() {
              return Ok(0);
            }
            if !read_in(&mut self.sock, tls.get_mut())? {
              return Err(io::ErrorKind::WouldBlock.into());
            }
          }
          Err(e) if matches!(e.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionAborted) => return Ok(0),
          Err(e) => return Err(e),
        }
      }
    }

    pub(crate) fn flush(&mut self, pending: &mut BytesMut) -> io::Result<bool> {
      let State::Ready(tls) = &mut self.state else { return Err(io::ErrorKind::NotConnected.into()) };
      if !pending.is_empty() {
        tls.write_all(pending)?;
        pending.clear();
      }
      write_out(&mut self.sock, tls.get_mut())
    }

    pub(crate) fn alpn(&self) -> Option<Vec<u8>> {
      match &self.state {
        State::Ready(tls) => tls.negotiated_alpn().ok().flatten(),
        _ => None,
      }
    }
  }
}

/// An in-memory connection for the shuttle models: the client side is an `Io`, the server side is driven by the model.
#[cfg(shuttle)]
pub(crate) mod model {
  use std::collections::VecDeque;
  use std::io;
  use std::task::Poll;

  use crate::notify::Waiter;
  use crate::sync::{Mutex, thread};
  use crate::wait::Interest;

  #[derive(Default)]
  struct State {
    to_client: VecDeque<u8>,
    to_server: Vec<u8>,
    eof: bool,
    client_waiters: Vec<Waiter>,
    server_waiters: Vec<Waiter>,
  }

  #[derive(Default)]
  pub(crate) struct Pipe {
    state: Mutex<State>,
  }

  fn wake(waiters: &mut Vec<Waiter>) {
    for mut w in waiters.drain(..) {
      w.wake();
    }
  }

  impl Pipe {
    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
      let mut st = self.state.lock();
      if st.to_client.is_empty() {
        return if st.eof { Ok(0) } else { Err(io::ErrorKind::WouldBlock.into()) };
      }
      let n = buf.len().min(st.to_client.len());
      for (slot, b) in buf.iter_mut().zip(st.to_client.drain(..n)) {
        *slot = b;
      }
      Ok(n)
    }

    pub(crate) fn send(&self, data: &[u8]) {
      let mut st = self.state.lock();
      st.to_server.extend_from_slice(data);
      wake(&mut st.server_waiters);
    }

    fn ready(st: &State, interest: Interest) -> bool {
      interest.write || !st.to_client.is_empty() || st.eof
    }

    pub(crate) fn block(&self, interest: Interest) {
      loop {
        {
          let mut st = self.state.lock();
          if Self::ready(&st, interest) {
            return;
          }
          st.client_waiters.push(Waiter::Thread(thread::current()));
        }
        thread::park();
      }
    }

    pub(crate) async fn wait_async(&self, interest: Interest) {
      std::future::poll_fn(|cx| {
        let mut st = self.state.lock();
        if Self::ready(&st, interest) {
          return Poll::Ready(());
        }
        st.client_waiters.push(Waiter::Task(cx.waker().clone()));
        Poll::Pending
      })
      .await
    }

    /// Server side: waits for request bytes and takes them all. Empty once the client side is closed.
    pub(crate) fn take_requests(&self) -> Vec<u8> {
      loop {
        {
          let mut st = self.state.lock();
          if !st.to_server.is_empty() || st.eof {
            return std::mem::take(&mut st.to_server);
          }
          st.server_waiters.push(Waiter::Thread(thread::current()));
        }
        thread::park();
      }
    }

    /// Server side: delivers response bytes to the client.
    pub(crate) fn respond(&self, data: &[u8]) {
      let mut st = self.state.lock();
      st.to_client.extend(data);
      wake(&mut st.client_waiters);
    }

    /// Ends both directions.
    pub(crate) fn close(&self) {
      let mut st = self.state.lock();
      st.eof = true;
      wake(&mut st.client_waiters);
      wake(&mut st.server_waiters);
    }
  }
}
