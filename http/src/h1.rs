//! HTTP/1.1 engine over `ureq-proto`.
//!
//! Every response reaches the caller, redirects included. Following a redirect is a policy above the engine. `ureq-proto` consumes 1xx responses before the final one, so this engine never emits `Event::Informational`.

use std::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use ureq_proto::BodyMode;
use ureq_proto::client::state::{Await100, Prepare, RecvBody, RecvResponse, SendBody};
use ureq_proto::client::{Await100Result, Call, RecvBodyResult, RecvResponseResult, SendRequestResult};

use crate::Error;
use crate::engine::{self, Capacity, ConnEvent, Event, ExchangeId, NetAction, OpenError, Output, RequestHead, ResponseHead};

const WRITE_CHUNK: usize = 16 * 1024;
/// How long a request with `Expect: 100-continue` waits for the 100 before sending its body anyway.
const EXPECT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// The most bytes a response head may take before it is refused.
const MAX_HEAD: usize = 64 * 1024;

enum State {
  Idle,
  Await100(Call<Await100>, Instant),
  SendBody(Call<SendBody>),
  RecvResponse(Call<RecvResponse>),
  RecvBody(Call<RecvBody>),
  Closed,
}

pub struct Engine {
  state: State,
  /// Request lines carry the whole URI, for a forward proxy.
  absolute: bool,
  current: Option<ExchangeId>,
  next_id: u64,
  scratch: Vec<u8>,
}

impl Engine {
  pub fn new() -> Self {
    Self { state: State::Idle, absolute: false, current: None, next_id: 1, scratch: vec![0; WRITE_CHUNK] }
  }

  /// An engine whose request lines carry the whole URI, as a forward proxy needs.
  pub fn absolute_form() -> Self {
    Self { absolute: true, ..Self::new() }
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

  /// Leaves the wait for `100 Continue`: the body may go, or the server answered without wanting it.
  fn end_await(&mut self, call: Call<Await100>, out: &mut Output) {
    match call.proceed() {
      Ok(Await100Result::SendBody(call)) => {
        self.state = State::SendBody(call);
        if let Some(id) = self.current {
          out.events.push((id, Event::Writable));
        }
      }
      Ok(Await100Result::RecvResponse(call)) => self.state = State::RecvResponse(call),
      Err(e) => self.fail(protocol(e), out),
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

/// `written` with the request line's target replaced by `uri`.
fn absolute_line(written: &[u8], uri: &str) -> Vec<u8> {
  let Some(end) = written.windows(2).position(|w| w == b"\r\n") else { return written.to_vec() };
  let line = String::from_utf8_lossy(&written[..end]);
  let mut parts = line.splitn(3, ' ');
  let (Some(method), Some(_), Some(version)) = (parts.next(), parts.next(), parts.next()) else { return written.to_vec() };
  let mut out = format!("{method} {uri} {version}").into_bytes();
  out.extend_from_slice(&written[end..]);
  out
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

  fn open(&mut self, head: RequestHead, now: Instant, out: &mut Output) -> Result<ExchangeId, OpenError> {
    match self.state {
      State::Idle => {}
      State::Closed => return Err(OpenError::Closed),
      _ => return Err(OpenError::Full),
    }
    let id = ExchangeId(self.next_id);
    self.next_id += 1;
    self.current = Some(id);

    let absolute = (self.absolute && head.method != http::Method::CONNECT).then(|| head.uri.to_string());
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
    if let Some(uri) = absolute {
      written = absolute_line(&written, &uri);
    }
    out.net.push(NetAction::Send(Bytes::from(written)));

    match call.proceed() {
      Ok(Some(SendRequestResult::SendBody(call))) => self.state = State::SendBody(call),
      Ok(Some(SendRequestResult::RecvResponse(call))) => self.state = State::RecvResponse(call),
      Ok(Some(SendRequestResult::Await100(call))) => self.state = State::Await100(call, now + EXPECT_WAIT),
      Ok(None) => self.fail(Error::Protocol("request head incomplete".into()), out),
      Err(e) => self.fail(protocol(e), out),
    }
    Ok(id)
  }

  fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize {
    if self.current != Some(id) {
      return 0;
    }
    if matches!(self.state, State::RecvResponse(_) | State::RecvBody(_)) {
      // The server answered before taking the body, so it is not sent; it counts as taken so the caller moves on.
      return data.len();
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
        State::Await100(mut call, deadline) => match call.try_read_100(input) {
          Ok(used) => {
            input.advance(used);
            if !call.can_keep_await_100() {
              self.end_await(call, out);
              continue;
            }
            self.state = State::Await100(call, deadline);
            if used == 0 {
              return;
            }
          }
          Err(e) => {
            self.fail(protocol(e), out);
            return;
          }
        },
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

  fn on_tick(&mut self, now: Instant, out: &mut Output) {
    if let State::Await100(_, deadline) = &self.state {
      if now >= *deadline {
        let State::Await100(call, _) = std::mem::replace(&mut self.state, State::Idle) else { unreachable!() };
        self.end_await(call, out);
      }
    }
  }

  fn next_deadline(&self) -> Option<Instant> {
    match &self.state {
      State::Await100(_, deadline) => Some(*deadline),
      _ => None,
    }
  }

  fn close(&mut self, out: &mut Output) {
    if let Some(id) = self.current.take() {
      out.events.push((id, Event::Reset(Error::Closed)));
    }
    self.shut(None, out);
  }
}
