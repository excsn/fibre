//! Response decompression, pushed one chunk at a time so blocking and async bodies share it.

use bytes::Bytes;

use crate::Error;

/// The `Accept-Encoding` sent when the caller sets none, or `None` with no decoder compiled in.
pub(crate) fn accept_encoding() -> Option<&'static str> {
  match (cfg!(feature = "gzip"), cfg!(feature = "zstd")) {
    (true, true) => Some("gzip, zstd"),
    (true, false) => Some("gzip"),
    (false, true) => Some("zstd"),
    (false, false) => None,
  }
}

pub(crate) enum Decoder {
  #[cfg(feature = "gzip")]
  Gzip { inner: flate2::write::GzDecoder<Vec<u8>>, fed: bool },
  #[cfg(feature = "zstd")]
  Zstd(Box<Zstd>),
  #[cfg(not(any(feature = "gzip", feature = "zstd")))]
  #[allow(dead_code)]
  Never(std::convert::Infallible),
}

#[cfg(feature = "zstd")]
pub(crate) struct Zstd {
  frame: ruzstd::decoding::FrameDecoder,
  input: Vec<u8>,
  started: bool,
  scratch: Vec<u8>,
}

/// The largest zstd frame header, which the first decode call must see whole.
#[cfg(feature = "zstd")]
const ZSTD_HEADER_MAX: usize = 18;
#[cfg(feature = "zstd")]
const ZSTD_SCRATCH: usize = 128 * 1024 + 21;

impl Decoder {
  /// The decoder for a `Content-Encoding` value, if one is compiled in.
  pub(crate) fn for_encoding(value: &str) -> Option<Decoder> {
    #[allow(clippy::match_single_binding)]
    match value.trim().to_ascii_lowercase().as_str() {
      #[cfg(feature = "gzip")]
      "gzip" | "x-gzip" => Some(Decoder::Gzip { inner: flate2::write::GzDecoder::new(Vec::new()), fed: false }),
      #[cfg(feature = "zstd")]
      "zstd" => Some(Decoder::Zstd(Box::new(Zstd {
        frame: ruzstd::decoding::FrameDecoder::new(),
        input: Vec::new(),
        started: false,
        scratch: vec![0; ZSTD_SCRATCH],
      }))),
      _ => None,
    }
  }

  /// Decodes `chunk` and returns what it produced, possibly nothing yet.
  pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Bytes, Error> {
    #[cfg(not(any(feature = "gzip", feature = "zstd")))]
    let _ = chunk;
    match self {
      #[cfg(feature = "gzip")]
      Decoder::Gzip { inner, fed } => {
        use std::io::Write;
        *fed = true;
        inner.write_all(chunk).map_err(|e| Error::Decode(format!("gzip: {e}")))?;
        Ok(Bytes::from(std::mem::take(inner.get_mut())))
      }
      #[cfg(feature = "zstd")]
      Decoder::Zstd(z) => {
        z.input.extend_from_slice(chunk);
        z.run(false)
      }
      #[cfg(not(any(feature = "gzip", feature = "zstd")))]
      Decoder::Never(never) => match *never {},
    }
  }

  /// Ends the stream and returns the rest. Fails on a truncated body.
  pub(crate) fn finish(&mut self) -> Result<Bytes, Error> {
    match self {
      #[cfg(feature = "gzip")]
      Decoder::Gzip { inner, fed } => {
        if !*fed {
          return Ok(Bytes::new());
        }
        inner.try_finish().map_err(|e| Error::Decode(format!("gzip: {e}")))?;
        Ok(Bytes::from(std::mem::take(inner.get_mut())))
      }
      #[cfg(feature = "zstd")]
      Decoder::Zstd(z) => z.run(true),
      #[cfg(not(any(feature = "gzip", feature = "zstd")))]
      Decoder::Never(never) => match *never {},
    }
  }
}

#[cfg(feature = "zstd")]
impl Zstd {
  fn run(&mut self, end: bool) -> Result<Bytes, Error> {
    let bad = |e: ruzstd::decoding::errors::FrameDecoderError| Error::Decode(format!("zstd: {e}"));
    let mut out = Vec::new();
    if !self.started {
      if self.input.is_empty() {
        return Ok(Bytes::new());
      }
      if self.input.len() < ZSTD_HEADER_MAX && !end {
        return Ok(Bytes::new());
      }
      self.started = true;
    }
    loop {
      if self.frame.is_finished() && self.frame.can_collect() == 0 {
        if self.input.is_empty() {
          break;
        }
        self.frame = ruzstd::decoding::FrameDecoder::new();
        if self.input.len() < ZSTD_HEADER_MAX && !end {
          self.started = false;
          break;
        }
      }
      let (read, written) = self.frame.decode_from_to(&self.input, &mut self.scratch).map_err(bad)?;
      self.input.drain(..read);
      out.extend_from_slice(&self.scratch[..written]);
      if read == 0 && written == 0 {
        break;
      }
    }
    if end && (!self.frame.is_finished() || !self.input.is_empty()) {
      return Err(Error::Decode("zstd: truncated body".into()));
    }
    Ok(Bytes::from(out))
  }
}
