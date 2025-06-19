// src/config/mod.rs
// This module handles configuration parsing and validation.

pub mod de; // Deserialization helpers
pub mod raw; // Structs directly mapping to YAML/JSON structure
pub mod processed; // Structs representing validated and processed configuration

// Re-export key structs if they become part of the public API (unlikely for raw/processed)
// For now, this module is mostly for internal use.