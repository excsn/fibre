//! A closed rendezvous handle stays closed for its async operations, as it
//! does for `try_send`, `try_recv` and the blocking calls.

#![allow(unused_mut)]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use fibre::error::{RecvError, SendError, TryRecvError};
use fibre::{mpmc, mpsc, spsc};

fn poll_once<F: Future>(f: std::pin::Pin<&mut F>) -> Poll<F::Output> {
  f.poll(&mut Context::from_waker(Waker::noop()))
}

macro_rules! closed_handle_tests {
  ($module:ident, $flavor:path) => {
    mod $module {
      use super::*;
      use $flavor as rv;

      #[test]
      fn a_closed_async_sender_fails_its_send_and_delivers_nothing() {
        let (tx, mut rx) = rv::rendezvous_async::<u32>();
        tx.close().unwrap();
        let mut send = pin!(tx.send(5));
        assert_eq!(poll_once(send.as_mut()), Poll::Ready(Err(SendError::Closed)));
        assert_eq!(rx.try_recv().ok(), None);
      }

      #[test]
      fn a_closed_async_receiver_fails_its_recv() {
        let (_tx, mut rx) = rv::rendezvous_async::<u32>();
        rx.close().unwrap();
        let mut recv = pin!(rx.recv());
        assert_eq!(poll_once(recv.as_mut()), Poll::Ready(Err(RecvError::Disconnected)));
      }

      #[test]
      fn an_async_sender_closed_after_its_send_was_made_fails_it() {
        let (tx, mut rx) = rv::rendezvous_async::<u32>();
        let mut send = pin!(tx.send(5));
        tx.close().unwrap();
        assert_eq!(poll_once(send.as_mut()), Poll::Ready(Err(SendError::Closed)));
        assert_eq!(rx.try_recv().ok(), None);
      }

      #[test]
      fn an_open_async_pair_still_hands_off() {
        let (tx, mut rx) = rv::rendezvous_async::<u32>();
        {
          let mut recv = pin!(rx.recv());
          assert!(poll_once(recv.as_mut()).is_pending());
          assert!(tx.try_send(9).is_ok());
          assert_eq!(poll_once(recv.as_mut()), Poll::Ready(Ok(9)));
        }
        let mut send = pin!(tx.send(4));
        assert!(poll_once(send.as_mut()).is_pending());
        assert_eq!(rx.try_recv(), Ok(4));
        assert_eq!(poll_once(send.as_mut()), Poll::Ready(Ok(())));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
      }
    }
  };
}

closed_handle_tests!(spsc_rendezvous, spsc::rendezvous);
closed_handle_tests!(mpsc_rendezvous, mpsc::rendezvous);
closed_handle_tests!(mpmc_rendezvous, mpmc::rendezvous);

#[test]
fn an_mpmc_async_receiver_closed_after_its_recv_was_made_fails_it() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<u32>();
  let mut recv = pin!(rx.recv());
  rx.close().unwrap();
  assert_eq!(poll_once(recv.as_mut()), Poll::Ready(Err(RecvError::Disconnected)));
  assert!(tx.try_send(1).is_err());
}
