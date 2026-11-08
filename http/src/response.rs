use bytes::Bytes;
use http::{HeaderMap, StatusCode, Version};

use crate::Error;
use crate::body::{AsyncBody, SyncBody};

pub struct Response<B> {
  status: StatusCode,
  version: Version,
  headers: HeaderMap,
  body: B,
}

impl<B> Response<B> {
  pub(crate) fn new(status: StatusCode, version: Version, headers: HeaderMap, body: B) -> Self {
    Self { status, version, headers, body }
  }

  pub(crate) fn map_body<C>(self, f: impl FnOnce(B) -> C) -> Response<C> {
    Response { status: self.status, version: self.version, headers: self.headers, body: f(self.body) }
  }

  pub fn status(&self) -> StatusCode {
    self.status
  }

  pub fn version(&self) -> Version {
    self.version
  }

  pub fn headers(&self) -> &HeaderMap {
    &self.headers
  }

  pub fn body_mut(&mut self) -> &mut B {
    &mut self.body
  }

  pub fn into_body(self) -> B {
    self.body
  }

  /// The response itself, or `Error::Status` for a 4xx or 5xx.
  pub fn error_for_status(self) -> Result<Self, Error> {
    if self.status.is_client_error() || self.status.is_server_error() {
      return Err(Error::Status(self.status.as_u16()));
    }
    Ok(self)
  }
}

impl Response<SyncBody> {
  pub fn to_async(self) -> Response<AsyncBody> {
    self.map_body(SyncBody::to_async)
  }

  pub fn bytes(self) -> Result<Bytes, Error> {
    self.body.bytes()
  }

  pub fn text(self) -> Result<String, Error> {
    String::from_utf8(self.bytes()?.to_vec()).map_err(|e| Error::Decode(e.to_string()))
  }

  #[cfg(feature = "json")]
  pub fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, Error> {
    serde_json::from_slice(&self.bytes()?).map_err(|e| Error::Decode(e.to_string()))
  }
}

impl Response<AsyncBody> {
  pub fn to_sync(self) -> Response<SyncBody> {
    self.map_body(AsyncBody::to_sync)
  }

  pub async fn bytes(self) -> Result<Bytes, Error> {
    self.body.bytes().await
  }

  pub async fn text(self) -> Result<String, Error> {
    String::from_utf8(self.bytes().await?.to_vec()).map_err(|e| Error::Decode(e.to_string()))
  }

  #[cfg(feature = "json")]
  pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, Error> {
    serde_json::from_slice(&self.bytes().await?).map_err(|e| Error::Decode(e.to_string()))
  }
}
