/// Resolves a fan-out strategy name to the helper that multiplies one handle
/// into N. `single` refuses N > 1, which is how a non-cloneable handle reports
/// that a cell is unsupported.
macro_rules! fan {
  (clone) => {
    $crate::channel::fan_clone
  };
  (single) => {
    $crate::channel::fan_single
  };
}

/// The `batch`, `send_batch` and `recv_batch` clauses are optional; omitting one
/// leaves the trait's looping default in place, which is what a caller without
/// that library's batch API would have to write.
macro_rules! sync_adapter {
  (
    name: $name:ident,
    sender: $sender:ty,
    receiver: $receiver:ty,
    senders: $sfan:ident,
    receivers: $rfan:ident,
    build: |$cap:ident| $build:expr,
    send: |$tx:ident, $item:ident| $send:expr,
    recv: |$rx:ident| $recv:expr,
    $(batch: $support:ident,)?
    $(send_batch: |$btx:ident, $bitems:ident| $send_batch:expr,)?
    $(recv_batch: |$brx:ident, $bout:ident, $bmax:ident| $recv_batch:expr,)?
  ) => {
    pub struct $name;

    impl $crate::channel::SyncChannel for $name {
      type Sender = $sender;
      type Receiver = $receiver;

      $(const BATCH: $crate::spec::BatchSupport = $crate::spec::BatchSupport::$support;)?

      fn build(
        cap: $crate::spec::Capacity,
        producers: usize,
        consumers: usize,
      ) -> $crate::channel::Handles<Self::Sender, Self::Receiver> {
        let (tx, rx) = {
          let $cap = cap;
          $build
        }?;
        Some((fan!($sfan)(tx, producers)?, fan!($rfan)(rx, consumers)?))
      }

      fn send(tx: &mut Self::Sender, item: $crate::channel::Payload) -> bool {
        let $tx = tx;
        let $item = item;
        $send
      }

      fn recv(rx: &mut Self::Receiver) -> Option<$crate::channel::Payload> {
        let $rx = rx;
        $recv
      }

      $(
        fn send_batch(
          tx: &mut Self::Sender,
          items: &mut Vec<$crate::channel::Payload>,
        ) -> bool {
          let $btx = tx;
          let $bitems = items;
          $send_batch
        }
      )?

      $(
        fn recv_batch(
          rx: &mut Self::Receiver,
          out: &mut Vec<$crate::channel::Payload>,
          max: usize,
        ) -> usize {
          let $brx = rx;
          let $bout = out;
          let $bmax = max;
          $recv_batch
        }
      )?
    }
  };
}

macro_rules! async_adapter {
  (
    name: $name:ident,
    sender: $sender:ty,
    receiver: $receiver:ty,
    senders: $sfan:ident,
    receivers: $rfan:ident,
    build: |$cap:ident| $build:expr,
    send: |$tx:ident, $item:ident| $send:expr,
    recv: |$rx:ident| $recv:expr,
    $(batch: $support:ident,)?
    $(send_batch: |$btx:ident, $bitems:ident| $send_batch:expr,)?
    $(recv_batch: |$brx:ident, $bout:ident, $bmax:ident| $recv_batch:expr,)?
  ) => {
    pub struct $name;

    impl $crate::channel::AsyncChannel for $name {
      type Sender = $sender;
      type Receiver = $receiver;

      $(const BATCH: $crate::spec::BatchSupport = $crate::spec::BatchSupport::$support;)?

      fn build(
        cap: $crate::spec::Capacity,
        producers: usize,
        consumers: usize,
      ) -> $crate::channel::Handles<Self::Sender, Self::Receiver> {
        let (tx, rx) = {
          let $cap = cap;
          $build
        }?;
        Some((fan!($sfan)(tx, producers)?, fan!($rfan)(rx, consumers)?))
      }

      fn send<'a>(
        tx: &'a mut Self::Sender,
        item: $crate::channel::Payload,
      ) -> impl ::std::future::Future<Output = bool> + Send + 'a {
        async move {
          let $tx = tx;
          let $item = item;
          $send
        }
      }

      fn recv<'a>(
        rx: &'a mut Self::Receiver,
      ) -> impl ::std::future::Future<Output = Option<$crate::channel::Payload>> + Send + 'a {
        async move {
          let $rx = rx;
          $recv
        }
      }

      $(
        fn send_batch<'a>(
          tx: &'a mut Self::Sender,
          items: &'a mut Vec<$crate::channel::Payload>,
        ) -> impl ::std::future::Future<Output = bool> + Send + 'a {
          async move {
            let $btx = tx;
            let $bitems = items;
            $send_batch
          }
        }
      )?

      $(
        fn recv_batch<'a>(
          rx: &'a mut Self::Receiver,
          out: &'a mut Vec<$crate::channel::Payload>,
          max: usize,
        ) -> impl ::std::future::Future<Output = usize> + Send + 'a {
          async move {
            let $brx = rx;
            let $bout = out;
            let $bmax = max;
            $recv_batch
          }
        }
      )?
    }
  };
}

pub(crate) use {async_adapter, fan, sync_adapter};
