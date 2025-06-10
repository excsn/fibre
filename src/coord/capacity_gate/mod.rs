mod futures;
mod mixed;

pub use mixed::{CapacityGate, AcquireFuture};
pub use futures::{CapacityGate as FuturesCapacityGate, OwnedPermitGuard as FuturesOwnedPermitGuard};