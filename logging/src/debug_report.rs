// This entire module will only be compiled when debug_assertions are enabled.
#![cfg(debug_assertions)]

use once_cell::sync::Lazy;
use parking_lot::{Mutex, MutexGuard};
use std::collections::HashMap;
use std::time::Instant;

use crate::LogValue;

// --- The Global Collector for the Debug Report Appender ---
pub(super) static DEBUG_REPORT_COLLECTOR: Lazy<Mutex<DebugReportData>> =
  Lazy::new(|| Mutex::new(Default::default()));

// --- New Structs for the Debug Report Appender ---
#[derive(Debug, Clone)]
pub(super) struct DebugEvent {
  // We capture less than LogEvent, but what's important for the report
  pub(super) timestamp: Instant,
  pub(super) level: tracing::Level,
  pub(super) target: String,
  pub(super) message: Option<String>,
  pub(super) fields: HashMap<String, LogValue>,
}

type DebugCounterKey = (String, String); // (target, counter_name)

#[derive(Default)]
pub(super) struct DebugReportData {
  pub(super) events: Vec<DebugEvent>,
  pub(super) counters: HashMap<DebugCounterKey, usize>,
  pub(super) start_time: Option<Instant>,
}

// --- The New Public Function ---

pub(super) fn print_report_logic(collector: &mut MutexGuard<DebugReportData>) {

  let start_time = if let Some(st) = collector.start_time {
    st
  } else {
    println!("\n--- Fibre Logging Debug Report ---");
    println!("[Events] No events recorded by a debug_report appender.");
    println!("\n--- End of Logging Debug Report ---");
    return;
  };

  println!("\n--- Fibre Logging Debug Report ---");

  // Print Events
  if collector.events.is_empty() {
    println!("\n[Events] No detailed events recorded.");
  } else {
    println!("\n[Events] Recorded Events ({}):", collector.events.len());
    // Sort events by timestamp
    collector.events.sort_by_key(|e| e.timestamp);
    for event in &collector.events {
      let time_since_start = event.timestamp.duration_since(start_time);
      println!(
        "  +{:<10.6}s [{:<5}] {:<25} - {} {:?}",
        time_since_start.as_secs_f64(),
        event.level.to_string(),
        event.target,
        event.message.as_deref().unwrap_or(""),
        event.fields
      );
    }
  }

  // Print Counters
  if collector.counters.is_empty() {
    println!("\n[Counters] No counters recorded.");
  } else {
    println!(
      "\n[Counters] Recorded Counters ({}):",
      collector.counters.len()
    );
    let mut sorted_counters: Vec<_> = collector.counters.iter().collect();
    sorted_counters.sort_by_key(|(k, _v)| *k);
    for ((target, name), count) in sorted_counters {
      println!(
        "  Target:{:<25} Counter:{:<30} Value: {}",
        target, name, count
      );
    }
  }
  println!("\n--- End of Logging Debug Report ---");
}

/// Prints a detailed, sorted report of all events and counters captured
/// by any `debug_report` appenders defined in the configuration.
pub fn print_debug_report() {
  let mut guard = DEBUG_REPORT_COLLECTOR.lock();
  print_report_logic(&mut guard);
}

/// Clears all data from the debug report collector.
pub fn clear_debug_report() {
  let mut collector = DEBUG_REPORT_COLLECTOR.lock();
  *collector = Default::default();
  println!("[fibre_logging] Debug report data cleared.");
}
