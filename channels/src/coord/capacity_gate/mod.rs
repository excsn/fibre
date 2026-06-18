mod futures;
mod mixed;

pub use futures::{
  CapacityGate as FuturesCapacityGate, OwnedPermitGuard as FuturesOwnedPermitGuard,
};
pub use mixed::{AcquireFuture, AcquireManyFuture, CapacityGate};

#[cfg(test)]
mod tests {

  use super::*;
  use std::sync::Arc;
  use std::thread;
  use std::time::Duration;

  /// Regression Test: A synchronous acquire attempt on a closed gate must
  /// return immediately instead of parking.
  ///
  /// Under the current buggy codebase, this test will FAIL because the thread
  /// deadlocks on `acquire_sync()`, causing `is_blocked` to be true.
  #[test]
  #[cfg(not(miri))] // Gate this timing-dependent non-blocking assertion under Miri
  fn test_capacity_gate_sync_regression() {
    let gate = Arc::new(CapacityGate::new(1));

    // 1. Consume the only available permit
    assert!(gate.try_acquire());

    // 2. Close the gate
    gate.close();

    // 3. Attempt a synchronous acquire on the closed gate
    let gate_clone = gate.clone();
    let handle = thread::spawn(move || {
      gate_clone.acquire_sync();
    });

    // 4. Give the thread time to execute
    thread::sleep(Duration::from_millis(100));

    // If fixed, the thread must have completed and exited.
    let is_blocked = !handle.is_finished();

    // Clean up the thread to prevent hanging the test runner on failure
    if is_blocked {
      gate.release();
    }
    handle.join().expect("Sender thread panicked on join");

    // ASSERT: The thread must NOT have blocked.
    // This assertion will FAIL on the buggy codebase.
    assert!(
      !is_blocked,
      "REGRESSION: acquire_sync() deadlocked permanently on a closed CapacityGate!"
    );
  }

  /// Regression Test: An asynchronous acquire attempt on a closed gate must
  /// resolve immediately instead of pending forever.
  ///
  /// Under the current buggy codebase, this test will FAIL because `acquire_async()`
  /// times out, causing `result.is_ok()` to be false.
  #[cfg(not(miri))]
  #[tokio::test]
  async fn test_capacity_gate_async_regression() {
    let gate = Arc::new(CapacityGate::new(1));

    // 1. Consume the only available permit
    assert!(gate.try_acquire());

    // 2. Close the gate
    gate.close();

    // 3. Attempt to acquire a permit asynchronously
    let acquire_fut = gate.acquire_async();

    // 4. Await with a timeout.
    let result = tokio::time::timeout(Duration::from_millis(100), acquire_fut).await;

    // ASSERT: The future must resolve immediately without timing out.
    // This assertion will FAIL on the buggy codebase.
    assert!(
      result.is_ok(),
      "REGRESSION: acquire_async() deadlocked and timed out on a closed CapacityGate!"
    );
  }
}
