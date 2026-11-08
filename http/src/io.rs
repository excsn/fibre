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
}

impl Io {
  pub(crate) fn fd(&self) -> RawFd {
    match self {
      Io::Plain(s) => s.as_raw_fd(),
      #[cfg(feature = "rustls")]
      Io::Tls(t) => t.0.as_raw_fd(),
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
    }
  }

  pub(crate) fn alpn(&self) -> Option<&[u8]> {
    match self {
      Io::Plain(_) => None,
      #[cfg(feature = "rustls")]
      Io::Tls(t) => t.1.alpn_protocol(),
    }
  }

  /// Whether a non-blocking peek on an idle connection finds it closed or holding bytes nobody asked for.
  pub(crate) fn idle_unusable(&self) -> bool {
    let mut byte = 0u8;
    let n = unsafe { libc::recv(self.fd(), (&mut byte as *mut u8).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
    n >= 0
  }
}
