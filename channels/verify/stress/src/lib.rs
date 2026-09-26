//! Real-hardware stress gates: long randomized-by-the-OS runs of the channels
//! with per-iteration timeouts, so a lost wakeup fails as a named hang instead
//! of a stuck process. Run them with `channels/verify/scripts/stress.sh`.
