//! HTTP/1.1 engine over `ureq-proto`.
//!
//! Every response reaches the caller, redirects included. Following a redirect is a policy above the engine. `ureq-proto` consumes 1xx responses before the final one, so this engine never emits `Event::Informational`.

use std::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use ureq_proto::BodyMode;
use ureq_proto::client::state::{Prepare, RecvBody, RecvResponse, SendBody};
use ureq_proto::client::{Await100Result, Call, RecvBodyResult, RecvResponseResult, SendRequestResult};

use crate::Error;
use crate::engine::{self, Capacity, ConnEvent, Event, ExchangeId, NetAction, OpenError, Output, RequestHead, ResponseHead};

const WRITE_CHUNK: usize = 16 * 1024;
/// The most bytes a response head may take before it is refused.
const MAX_HEAD: usize = 64 * 1024;

enum State {
  Idle,
  SendBody(Call<SendBody>),
  RecvResponse(Call<RecvResponse>),
  RecvBody(Call<RecvBody>),
  Closed,
}

pub struct Engine {
  state: State,
  current: Option<ExchangeId>,
  next_id: u64,
  scratch: Vec<u8>,
}

impl Engine {
  pub fn new() -> Self {
    Self { state: State::Idle, current: None, next_id: 1, scratch: vec![0; WRITE_CHUNK] }
  }

  fn fail(&mut self, error: Error, out: &mut Output) {
    if let Some(id) = self.current.take() {
      out.events.push((id, Event::Reset(error.clone())));
    }
    self.shut(Some(error), out);
  }

  fn shut(&mut self, error: Option<Error>, out: &mut Output) {
    if !matches!(self.state, State::Closed) {
      self.state = State::Closed;
      self.current = None;
      out.net.push(NetAction::Close);
      out.conn.push(ConnEvent::Closed(error));
    }
  }

  fn finish(&mut self, must_close: bool, out: &mut Output) {
    if let Some(id) = self.current.take() {
      out.events.push((id, Event::End));
    }
    if must_close {
      self.shut(None, out);
    } else {
      self.state = State::Idle;
    }
  }

  fn after_head(&mut self, result: Option<RecvResponseResult>, out: &mut Output) {
    match result {
      Some(RecvResponseResult::RecvBody(call)) => self.state = State::RecvBody(call),
      Some(RecvResponseResult::Redirect(call)) => self.finish(call.must_close_connection(), out),
      Some(RecvResponseResult::Cleanup(call)) => self.finish(call.must_close_connection(), out),
      None => self.fail(Error::Protocol("response could not proceed".into()), out),
    }
  }

  fn after_body(&mut self, result: Option<RecvBodyResult>, out: &mut Output) {
    match result {
      Some(RecvBodyResult::Redirect(call)) => self.finish(call.must_close_connection(), out),
      Some(RecvBodyResult::Cleanup(call)) => self.finish(call.must_close_connection(), out),
      None => self.fail(Error::Protocol("body could not proceed".into()), out),
    }
  }
}

impl Default for Engine {
  fn default() -> Self {
    Self::new()
  }
}

fn protocol(e: ureq_proto::Error) -> Error {
  Error::Protocol(e.to_string())
}

fn request(head: RequestHead) -> Result<Call<Prepare>, ureq_proto::Error> {
  let mut builder = http::Request::builder().method(head.method).uri(head.uri).version(head.version);
  if let Some(headers) = builder.headers_mut() {
    *headers = head.headers;
  }
  Call::new(builder.body(()).expect("request parts were already valid"))
}

impl engine::Engine for Engine {
  fn capacity(&self) -> Capacity {
    Capacity::Unique { available: matches!(self.state, State::Idle) }
  }

  fn open(&mut self, head: RequestHead, out: &mut Output) -> Result<ExchangeId, OpenError> {
    match self.state {
      State::Idle => {}
      State::Closed => return Err(OpenError::Closed),
      _ => return Err(OpenError::Full),
    }
    let id = ExchangeId(self.next_id);
    self.next_id += 1;
    self.current = Some(id);

    let mut call = match request(head) {
      Ok(prepare) => prepare.proceed(),
      Err(e) => {
        self.fail(protocol(e), out);
        return Ok(id);
      }
    };
    let mut written = Vec::new();
    while !call.can_proceed() {
      match call.write(&mut self.scratch) {
        Ok(n) => written.extend_from_slice(&self.scratch[..n]),
        Err(e) => {
          self.fail(protocol(e), out);
          return Ok(id);
        }
      }
    }
    out.net.push(NetAction::Send(Bytes::from(written)));

    match call.proceed() {
      Ok(Some(SendRequestResult::SendBody(call))) => self.state = State::SendBody(call),
      Ok(Some(SendRequestResult::RecvResponse(call))) => self.state = State::RecvResponse(call),
      // Expect: 100-continue sends the body at once. Waiting for the 100 needs a deadline from open, which the trait does not give yet.
      Ok(Some(SendRequestResult::Await100(call))) => match call.proceed() {
        Ok(Await100Result::SendBody(call)) => self.state = State::SendBody(call),
        Ok(Await100Result::RecvResponse(call)) => self.state = State::RecvResponse(call),
        Err(e) => self.fail(protocol(e), out),
      },
      Ok(None) => self.fail(Error::Protocol("request head incomplete".into()), out),
      Err(e) => self.fail(protocol(e), out),
    }
    Ok(id)
  }

  fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize {
    if self.current != Some(id) {
      return 0;
    }
    let State::SendBody(call) = &mut self.state else {
      return 0;
    };
    let mut taken = 0;
    let mut written = Vec::new();
    let mut failed = None;
    while taken < data.len() {
      match call.write(&data[taken..], &mut self.scratch) {
        Ok((used, n)) => {
          taken += used;
          written.extend_from_slice(&self.scratch[..n]);
          if used == 0 && n == 0 {
            break;
          }
        }
        Err(e) => {
          failed = Some(protocol(e));
          break;
        }
      }
    }
    if failed.is_none() && end {
      match call.write(&[], &mut self.scratch) {
        Ok((_, n)) => written.extend_from_slice(&self.scratch[..n]),
        Err(e) => failed = Some(protocol(e)),
      }
    }
    if !written.is_empty() {
      out.net.push(NetAction::Send(Bytes::from(written)));
    }
    if let Some(error) = failed {
      self.fail(error, out);
      return taken;
    }
    if end && call.can_proceed() {
      let State::SendBody(call) = std::mem::replace(&mut self.state, State::Idle) else { unreachable!() };
      match call.proceed() {
        Some(call) => self.state = State::RecvResponse(call),
        None => self.fail(Error::Protocol("request body incomplete".into()), out),
      }
    }
    taken
  }

  fn consumed(&mut self, _id: ExchangeId, _n: usize, _out: &mut Output) {}

  fn reset(&mut self, id: ExchangeId, out: &mut Output) {
    if self.current == Some(id) {
      self.current = None;
      self.shut(None, out);
    }
  }

  fn on_bytes(&mut self, input: &mut BytesMut, out: &mut Output) {
    loop {
      match std::mem::replace(&mut self.state, State::Idle) {
        State::RecvResponse(mut call) => match call.try_response(input, false) {
          Ok((used, Some(response))) => {
            input.advance(used);
            let (parts, ()) = response.into_parts();
            if let Some(id) = self.current {
              out.events.push((id, Event::Head(ResponseHead { status: parts.status, version: parts.version, headers: parts.headers })));
            }
            self.after_head(call.proceed(), out);
          }
          Ok((used, None)) => {
            input.advance(used);
            if used == 0 && input.len() > MAX_HEAD {
              self.fail(Error::Protocol(format!("response head over {MAX_HEAD} bytes")), out);
              return;
            }
            self.state = State::RecvResponse(call);
            if used == 0 {
              return;
            }
          }
          Err(e) => {
            self.fail(protocol(e), out);
            return;
          }
        },
        State::RecvBody(mut call) => {
          let close_delimited = matches!(call.body_mode(), BodyMode::CloseDelimited);
          if !close_delimited && call.can_proceed() {
            self.after_body(call.proceed(), out);
            continue;
          }
          if input.is_empty() {
            self.state = State::RecvBody(call);
            return;
          }
          let mut decoded = vec![0u8; input.len().max(1)];
          match call.read(input, &mut decoded) {
            Ok((used, n)) => {
              input.advance(used);
              if n > 0 {
                decoded.truncate(n);
                if let Some(id) = self.current {
                  out.events.push((id, Event::Data(Bytes::from(decoded))));
                }
              }
              let ended = !close_delimited && call.can_proceed();
              self.state = State::RecvBody(call);
              if ended {
                continue;
              }
              if used == 0 && n == 0 {
                return;
              }
            }
            Err(e) => {
              self.fail(protocol(e), out);
              return;
            }
          }
        }
        State::Idle if !input.is_empty() => {
          input.clear();
          self.shut(None, out);
          return;
        }
        other => {
          self.state = other;
          return;
        }
      }
    }
  }

  fn on_eof(&mut self, out: &mut Output) {
    match std::mem::replace(&mut self.state, State::Idle) {
      State::RecvBody(call) if matches!(call.body_mode(), BodyMode::CloseDelimited) => self.finish(true, out),
      State::Idle => self.shut(None, out),
      State::Closed => self.state = State::Closed,
      _ => self.fail(Error::Closed, out),
    }
  }

  fn on_tick(&mut self, _now: Instant, _out: &mut Output) {}

  fn next_deadline(&self) -> Option<Instant> {
    None
  }

  fn close(&mut self, out: &mut Output) {
    if let Some(id) = self.current.take() {
      out.events.push((id, Event::Reset(Error::Closed)));
    }
    self.shut(None, out);
  }
}
