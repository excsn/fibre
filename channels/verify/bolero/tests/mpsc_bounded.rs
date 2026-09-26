//! Single-threaded op sequences on the bounded MPSC against a `VecDeque`.
//! `try_send` verifies Full against the drained counter, so on one thread it
//! is exact: it fails if and only if the model holds `cap` items.

use std::collections::VecDeque;

use bolero::{check, TypeGenerator};
use fibre::error::{TryRecvError, TrySendError};
use fibre::mpsc::bounded;

fn iterations() -> usize {
  std::env::var("FIBRE_BOLERO_ITERATIONS")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(10_000)
}

#[derive(Debug, Clone, TypeGenerator)]
enum Op {
  TrySend(u16),
  TryRecv,
  Len,
  IsFull,
  IsEmpty,
}

#[test]
fn sync_ops_match_model() {
  check!()
    .with_iterations(iterations())
    .with_type::<(u8, Vec<Op>)>()
    .for_each(|(cap, ops)| {
      let cap = usize::from(cap % 8) + 1;
      let (tx, rx) = bounded::<u16>(cap);
      let mut model = VecDeque::new();
      for op in ops {
        match *op {
          Op::TrySend(v) => match tx.try_send(v) {
            Ok(()) => {
              assert!(model.len() < cap, "accepted past capacity {cap}");
              model.push_back(v);
            }
            Err(TrySendError::Full(back)) => {
              assert_eq!(back, v);
              assert_eq!(model.len(), cap, "reported Full below capacity {cap}");
            }
            Err(e) => panic!("unexpected {e:?}"),
          },
          Op::TryRecv => match rx.try_recv() {
            Ok(v) => assert_eq!(Some(v), model.pop_front()),
            Err(TryRecvError::Empty) => assert!(model.is_empty(), "Empty with {} queued", model.len()),
            Err(e) => panic!("unexpected {e:?}"),
          },
          Op::Len => assert_eq!(rx.len(), model.len()),
          Op::IsFull => assert_eq!(tx.is_full(), model.len() == cap),
          Op::IsEmpty => assert_eq!(rx.is_empty(), model.is_empty()),
        }
      }
    });
}

#[test]
fn async_handles_ops_match_model() {
  check!()
    .with_iterations(iterations())
    .with_type::<(u8, Vec<Op>)>()
    .for_each(|(cap, ops)| {
      let cap = usize::from(cap % 8) + 1;
      let (tx, rx) = bounded::<u16>(cap);
      let (tx, rx) = (tx.to_async(), rx.to_async());
      let mut model = VecDeque::new();
      for op in ops {
        match *op {
          Op::TrySend(v) => match tx.try_send(v) {
            Ok(()) => {
              assert!(model.len() < cap, "accepted past capacity {cap}");
              model.push_back(v);
            }
            Err(TrySendError::Full(back)) => {
              assert_eq!(back, v);
              assert_eq!(model.len(), cap, "reported Full below capacity {cap}");
            }
            Err(e) => panic!("unexpected {e:?}"),
          },
          Op::TryRecv => match rx.try_recv() {
            Ok(v) => assert_eq!(Some(v), model.pop_front()),
            Err(TryRecvError::Empty) => assert!(model.is_empty(), "Empty with {} queued", model.len()),
            Err(e) => panic!("unexpected {e:?}"),
          },
          Op::Len => assert_eq!(rx.len(), model.len()),
          Op::IsFull => assert_eq!(tx.is_full(), model.len() == cap),
          Op::IsEmpty => assert_eq!(rx.is_empty(), model.is_empty()),
        }
      }
    });
}
