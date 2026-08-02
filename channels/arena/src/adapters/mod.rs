//! One thin adapter per library. Everything they share (workload, timing,
//! reporting) lives outside; an adapter only says how to build handles and how
//! to move one item.
//!
//! Capacity-specific constructors get their own adapter when the library uses a
//! distinct handle type for them (fibre's rendezvous channels), because the
//! trait fixes one sender/receiver type per adapter. Their capacity support is
//! disjoint, so both still report under one library name.

pub mod macros;

pub mod async_channel_ch;
pub mod crossbeam_ch;
pub mod fibre_ch;
pub mod flume_ch;
pub mod kanal_ch;
pub mod oneshot_ch;
pub mod std_ch;
pub mod tokio_ch;
