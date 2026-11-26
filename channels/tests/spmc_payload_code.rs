//! A payload's `Drop` runs inside the spmc sender, when a slot's value from the
//! lap before is replaced. These tests pin what that code may do: panic, or
//! send again on the same sender.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use fibre::spmc;

thread_local! {
  static DROPS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
  static PANIC_ON: Cell<Option<u32>> = const { Cell::new(None) };
  static SEND_ON: Cell<Option<u32>> = const { Cell::new(None) };
  static TX: RefCell<Option<Rc<spmc::BoundedSyncSender<Item>>>> = const { RefCell::new(None) };
}

struct Item(u32);

impl Clone for Item {
  fn clone(&self) -> Self {
    Item(self.0 + 1000)
  }
}

impl Drop for Item {
  fn drop(&mut self) {
    DROPS.with(|d| *d.borrow_mut().entry(self.0).or_insert(0) += 1);
    if PANIC_ON.get() == Some(self.0) {
      PANIC_ON.set(None);
      panic!("payload drop panics");
    }
    if SEND_ON.get() == Some(self.0) {
      SEND_ON.set(None);
      let tx = TX.with(|t| t.borrow().clone()).expect("sender registered");
      assert!(tx.try_send(Item(77)).is_ok());
    }
  }
}

fn drops_of(id: u32) -> u32 {
  DROPS.with(|d| d.borrow().get(&id).copied().unwrap_or(0))
}

fn reset() {
  DROPS.with(|d| d.borrow_mut().clear());
  PANIC_ON.set(None);
  SEND_ON.set(None);
  TX.with(|t| *t.borrow_mut() = None);
}

fn drain(rx: &mut spmc::BoundedSyncReceiver<Item>) -> Vec<u32> {
  let mut got = Vec::new();
  while let Ok(item) = rx.try_recv() {
    got.push(item.0 - 1000);
  }
  got
}

#[test]
fn a_value_whose_drop_panics_during_try_send_is_dropped_once() {
  reset();
  let (tx, mut rx) = spmc::bounded::<Item>(1);
  tx.try_send(Item(1)).unwrap();
  assert_eq!(drain(&mut rx), vec![1]);
  PANIC_ON.set(Some(1));
  assert!(catch_unwind(AssertUnwindSafe(|| tx.try_send(Item(2)))).is_err());
  assert_eq!(drain(&mut rx), vec![2]);
  tx.try_send(Item(3)).unwrap();
  assert_eq!(drain(&mut rx), vec![3]);
  drop(tx);
  drop(rx);
  assert_eq!(drops_of(1), 1);
  assert_eq!(drops_of(2), 1);
  assert_eq!(drops_of(3), 1);
}

#[test]
fn a_value_whose_drop_panics_during_an_async_try_send_is_dropped_once() {
  reset();
  let (mut tx, rx) = spmc::bounded_async::<Item>(1);
  let mut rx = rx.to_sync();
  tx.try_send(Item(1)).unwrap();
  assert_eq!(drain(&mut rx), vec![1]);
  PANIC_ON.set(Some(1));
  assert!(catch_unwind(AssertUnwindSafe(|| tx.try_send(Item(2)))).is_err());
  assert_eq!(drain(&mut rx), vec![2]);
  drop(tx);
  drop(rx);
  assert_eq!(drops_of(1), 1);
  assert_eq!(drops_of(2), 1);
}

#[test]
fn a_value_whose_drop_panics_during_try_send_batch_is_dropped_once() {
  reset();
  let (tx, mut rx) = spmc::bounded::<Item>(2);
  assert_eq!(tx.try_send_batch(vec![Item(1), Item(2)]).ok(), Some(2));
  assert_eq!(drain(&mut rx), vec![1, 2]);
  PANIC_ON.set(Some(1));
  assert!(catch_unwind(AssertUnwindSafe(|| tx.try_send_batch(vec![Item(3), Item(4)]))).is_err());
  assert_eq!(drain(&mut rx), Vec::<u32>::new());
  assert_eq!(tx.try_send_batch(vec![Item(5), Item(6)]).ok(), Some(2));
  assert_eq!(drain(&mut rx), vec![5, 6]);
  drop(tx);
  drop(rx);
  for id in [1, 2, 3, 4, 5, 6] {
    assert_eq!(drops_of(id), 1, "value {id}");
  }
}

#[test]
fn a_send_from_a_replaced_values_drop_is_delivered_after_the_send_that_replaced_it() {
  reset();
  let (tx, mut rx) = spmc::bounded::<Item>(2);
  let tx = Rc::new(tx);
  TX.with(|t| *t.borrow_mut() = Some(tx.clone()));
  tx.try_send(Item(1)).unwrap();
  tx.try_send(Item(2)).unwrap();
  assert_eq!(drain(&mut rx), vec![1, 2]);
  SEND_ON.set(Some(1));
  tx.try_send(Item(3)).unwrap();
  TX.with(|t| *t.borrow_mut() = None);
  assert_eq!(drain(&mut rx), vec![3, 77]);
  drop(tx);
  drop(rx);
  for id in [1, 2, 3, 77] {
    assert_eq!(drops_of(id), 1, "value {id}");
  }
}

#[test]
fn a_send_from_a_replaced_values_drop_during_a_batch_is_delivered_before_the_batch() {
  reset();
  let (tx, mut rx) = spmc::bounded::<Item>(4);
  let tx = Rc::new(tx);
  TX.with(|t| *t.borrow_mut() = Some(tx.clone()));
  assert_eq!(tx.try_send_batch(vec![Item(1), Item(2), Item(3), Item(4)]).ok(), Some(4));
  assert_eq!(drain(&mut rx), vec![1, 2, 3, 4]);
  SEND_ON.set(Some(1));
  assert_eq!(tx.try_send_batch(vec![Item(10), Item(11), Item(12)]).ok(), Some(3));
  TX.with(|t| *t.borrow_mut() = None);
  assert_eq!(drain(&mut rx), vec![77, 10, 11, 12]);
  drop(tx);
  drop(rx);
  for id in [1, 2, 3, 4, 10, 11, 12, 77] {
    assert_eq!(drops_of(id), 1, "value {id}");
  }
}
