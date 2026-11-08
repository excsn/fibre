//! Response bodies. `SyncBody` and `AsyncBody` hold the same exchange and switch with `to_async` and `to_sync`.

use std::io::{self, Read};
use std::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use http::HeaderMap;

use crate::Error;
use crate::decode::Decoder;
use crate::engine::{Event, ExchangeId};
use crate::pool::Lease;

pub(crate) struct BodyState {
  lease: Option<Lease>,
  id: ExchangeId,
  deadline: Option<Instant>,
  done: bool,
  leftover: Bytes,
  trailers: Option<HeaderMap>,
  decoder: Option<Decoder>,
  limit: Option<usize>,
  received: usize,
}

impl BodyState {
  pub(crate) fn new(lease: Lease, id: ExchangeId, deadline: Option<Instant>, empty: bool, decoder: Option<Decoder>, limit: Option<usize>) -> Self {
    let mut state = Self { lease: Some(lease), id, deadline, done: false, leftover: Bytes::new(), trailers: None, decoder, limit, received: 0 };
    if empty {
      state.drain_queued();
    }
    state
  }

  /// Takes queued events without waiting. Returns whether the body reached its end.
  fn drain_queued(&mut self) -> bool {
    let Some(lease) = self.lease.as_ref() else { return true };
    while let Some(event) = lease.conn.try_event(self.id) {
      match event {
        Event::End => {
          self.finish();
          return true;
        }
        Event::Reset(_) => {
          self.done = true;
          return false;
        }
        Event::Trailers(t) => self.trailers = Some(t),
        _ => {}
      }
    }
    false
  }

  fn finish(&mut self) {
    self.done = true;
    if let Some(lease) = self.lease.take() {
      lease.release();
    }
  }

  fn fail(&mut self, e: Error) -> Error {
    self.done = true;
    if let Some(lease) = self.lease.take() {
      lease.conn.reset(self.id);
      lease.release();
    }
    e
  }

  fn counted(&mut self, out: Bytes) -> Option<Result<Option<Bytes>, Error>> {
    self.received += out.len();
    if self.limit.is_some_and(|l| self.received > l) {
      return Some(Err(self.fail(Error::TooLarge)));
    }
    Some(Ok(Some(out)))
  }

  fn on(&mut self, event: Result<Event, Error>) -> Option<Result<Option<Bytes>, Error>> {
    match event {
      Ok(Event::Data(b)) => match self.decoder.as_mut() {
        None => self.counted(b),
        Some(d) => match d.push(&b) {
          Ok(out) if out.is_empty() => None,
          Ok(out) => self.counted(out),
          Err(e) => Some(Err(self.fail(e))),
        },
      },
      Ok(Event::Trailers(t)) => {
        self.trailers = Some(t);
        None
      }
      Ok(Event::End) => {
        let tail = match self.decoder.as_mut().map(Decoder::finish) {
          Some(Err(e)) => return Some(Err(self.fail(e))),
          Some(Ok(tail)) => tail,
          None => Bytes::new(),
        };
        if tail.is_empty() {
          self.finish();
          return Some(Ok(None));
        }
        let counted = self.counted(tail);
        if self.lease.is_some() {
          self.finish();
        }
        counted
      }
      Ok(Event::Reset(e)) | Err(e) => Some(Err(self.fail(e))),
      Ok(_) => None,
    }
  }

  /// Reads and discards up to `limit` bytes so a short body leaves its connection reusable.
  pub(crate) fn drain_sync(mut self, limit: usize) {
    let mut read = 0;
    while read <= limit {
      match self.chunk_sync() {
        Ok(Some(c)) => read += c.len(),
        _ => return,
      }
    }
  }

  pub(crate) async fn drain_async(mut self, limit: usize) {
    let mut read = 0;
    while read <= limit {
      match self.chunk_async().await {
        Ok(Some(c)) => read += c.len(),
        _ => return,
      }
    }
  }

  fn chunk_sync(&mut self) -> Result<Option<Bytes>, Error> {
    if !self.leftover.is_empty() {
      return Ok(Some(std::mem::take(&mut self.leftover)));
    }
    loop {
      if self.done {
        return Ok(None);
      }
      let Some(lease) = self.lease.as_ref() else { return Ok(None) };
      let event = lease.conn.next_event_sync(self.id, self.deadline);
      if let Some(r) = self.on(event) {
        return r;
      }
    }
  }

  async fn chunk_async(&mut self) -> Result<Option<Bytes>, Error> {
    if !self.leftover.is_empty() {
      return Ok(Some(std::mem::take(&mut self.leftover)));
    }
    loop {
      if self.done {
        return Ok(None);
      }
      let Some(lease) = self.lease.as_ref() else { return Ok(None) };
      let event = lease.conn.next_event_async(self.id, self.deadline).await;
      if let Some(r) = self.on(event) {
        return r;
      }
    }
  }
}

impl Drop for BodyState {
  fn drop(&mut self) {
    if !self.done && self.leftover.is_empty() && self.drain_queued() {
      return;
    }
    if !self.done || self.lease.is_some() {
      if let Some(lease) = self.lease.take() {
        lease.conn.reset(self.id);
        lease.release();
      }
    }
  }
}

pub struct SyncBody {
  state: BodyState,
}

pub struct AsyncBody {
  state: BodyState,
}

impl SyncBody {
  pub(crate) fn from_state(state: BodyState) -> Self {
    Self { state }
  }

  pub fn to_async(self) -> AsyncBody {
    AsyncBody { state: self.state }
  }

  /// The next chunk, or `None` at the end.
  pub fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
    self.state.chunk_sync()
  }

  pub fn bytes(mut self) -> Result<Bytes, Error> {
    let mut all = BytesMut::new();
    while let Some(c) = self.chunk()? {
      all.extend_from_slice(&c);
    }
    Ok(all.freeze())
  }

  /// Trailers, once the body has been read to the end.
  pub fn trailers(&self) -> Option<&HeaderMap> {
    self.state.trailers.as_ref()
  }
}

impl Read for SyncBody {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    if self.state.leftover.is_empty() {
      match self.state.chunk_sync() {
        Ok(Some(c)) => self.state.leftover = c,
        Ok(None) => return Ok(0),
        Err(e) => return Err(io::Error::other(e)),
      }
    }
    let n = buf.len().min(self.state.leftover.len());
    buf[..n].copy_from_slice(&self.state.leftover[..n]);
    self.state.leftover.advance(n);
    Ok(n)
  }
}

impl AsyncBody {
  pub(crate) fn from_state(state: BodyState) -> Self {
    Self { state }
  }

  pub fn to_sync(self) -> SyncBody {
    SyncBody { state: self.state }
  }

  /// The next chunk, or `None` at the end.
  pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
    self.state.chunk_async().await
  }

  pub async fn bytes(mut self) -> Result<Bytes, Error> {
    let mut all = BytesMut::new();
    while let Some(c) = self.chunk().await? {
      all.extend_from_slice(&c);
    }
    Ok(all.freeze())
  }

  /// Trailers, once the body has been read to the end.
  pub fn trailers(&self) -> Option<&HeaderMap> {
    self.state.trailers.as_ref()
  }
}
