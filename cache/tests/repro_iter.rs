//! Test suite to reproduce suspected iterator hang bug when items expire during iteration
//!
//! Bug Hypothesis: fibre_cache::Cache::iter() enters an infinite loop when:
//! 1. Items expire during iteration (TTL boundary condition)
//! 2. Cache is nearly empty (2-3 items)
//! 3. Janitor is actively cleaning up expired items
//!
//! Expected symptom: 100% CPU usage, iterator never completes

use fibre_cache::{CacheBuilder, builder::TimerWheelMode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

// ======================================================================================
// Test 1: TTL Expiration During Iteration (Most Likely Root Cause)
// ======================================================================================

#[tokio::test]
async fn test_iter_hangs_on_ttl_expiration_during_iteration() {
  println!("\n=== Test 1: TTL Expiration During Iteration ===");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  // Insert 3 items
  cache.insert(1, "job_1".to_string(), 1);
  cache.insert(2, "job_2".to_string(), 1);
  cache.insert(3, "job_3".to_string(), 1);

  println!("Inserted 3 items, waiting 90ms (close to 100ms TTL)...");

  // Wait until items are about to expire
  tokio::time::sleep(Duration::from_millis(90)).await;

  // Set up hang detection
  let hung = Arc::new(AtomicBool::new(false));
  let hung_clone = hung.clone();
  let iteration_count = Arc::new(AtomicUsize::new(0));
  let count_clone = iteration_count.clone();

  let iteration_task = tokio::task::spawn_blocking(move || {
    let start = std::time::Instant::now();
    let mut count = 0;

    println!("Starting iteration...");

    for (_key, _value) in cache.iter() {
      count += 1;
      count_clone.store(count, Ordering::SeqCst);

      // Safety: if we loop more than 10 times on 3 items, we're in trouble
      if count > 10 {
        hung_clone.store(true, Ordering::SeqCst);
        println!("ERROR: Iterated {} times on 3 items!", count);
        return count;
      }

      // Safety: if iteration takes too long, bail
      if start.elapsed() > Duration::from_secs(2) {
        hung_clone.store(true, Ordering::SeqCst);
        println!("ERROR: Iteration taking too long: {:?}", start.elapsed());
        return count;
      }
    }

    println!(
      "Iteration completed: {} items in {:?}",
      count,
      start.elapsed()
    );
    count
  });

  // Give it a reasonable timeout
  match tokio::time::timeout(Duration::from_secs(5), iteration_task).await {
    Ok(Ok(count)) => {
      let is_hung = hung.load(Ordering::SeqCst);
      assert!(!is_hung, "Iterator entered infinite loop!");
      assert!(count <= 3, "Should iterate at most 3 items, got {}", count);
      println!("✓ Test passed: {} iterations", count);
    }
    Ok(Err(e)) => panic!("Iterator task panicked: {:?}", e),
    Err(_) => {
      let count = iteration_count.load(Ordering::SeqCst);
      panic!(
        "Iterator timed out after {} iterations - INFINITE LOOP DETECTED!",
        count
      );
    }
  }
}

// ======================================================================================
// Test 2: Concurrent Janitor Cleanup During Iteration
// ======================================================================================

#[tokio::test]
async fn test_iter_with_concurrent_janitor_expiration() {
  println!("\n=== Test 2: Concurrent Janitor Cleanup ===");

  let cache = Arc::new(
    CacheBuilder::<u32, String>::new()
      .capacity(100)
      .time_to_live(Duration::from_millis(50))
      .timer_mode(TimerWheelMode::HighPrecisionShortLived)
      .janitor_tick_interval(Duration::from_millis(10)) // Aggressive cleanup
      .build()
      .unwrap(),
  );

  // Insert items
  println!("Inserting 5 items with 50ms TTL...");
  for i in 0..5 {
    cache.insert(i, format!("job_{}", i), 1);
  }

  // Wait for items to be on edge of expiration
  println!("Waiting 45ms (edge of expiration)...");
  tokio::time::sleep(Duration::from_millis(45)).await;

  println!("Starting iteration while janitor is actively cleaning...");

  let cache_clone = cache.clone();
  let iter_count = Arc::new(AtomicUsize::new(0));
  let count_clone = iter_count.clone();

  let iter_task = tokio::task::spawn_blocking(move || {
    let start = std::time::Instant::now();
    let mut count = 0;

    for (_k, _v) in cache_clone.iter() {
      count += 1;
      count_clone.store(count, Ordering::SeqCst);

      if count > 100 {
        println!("ERROR: Iterated {} times on 5 items!", count);
        return count;
      }

      if start.elapsed() > Duration::from_secs(2) {
        println!("ERROR: Iteration took {:?}", start.elapsed());
        return count;
      }
    }

    println!("Completed: {} items in {:?}", count, start.elapsed());
    count
  });

  match tokio::time::timeout(Duration::from_secs(5), iter_task).await {
    Ok(Ok(count)) => {
      println!("✓ Test passed: {} iterations", count);
      assert!(count <= 5, "Should iterate at most 5 items");
    }
    Ok(Err(e)) => panic!("Task panicked: {:?}", e),
    Err(_) => {
      let count = iter_count.load(Ordering::SeqCst);
      panic!("Iterator hung indefinitely after {} iterations!", count);
    }
  }
}

// ======================================================================================
// Test 3: Edge Case - Exactly 2 Expiring Items (Production Scenario)
// ======================================================================================

#[test]
fn test_iter_edge_case_two_expiring_items() {
  println!("\n=== Test 3: Two Expiring Items (Production Scenario) ===");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  // Insert exactly 2 items (matches production scenario)
  println!("Inserting 2 items...");
  cache.insert(1, "job_a".to_string(), 1);
  cache.insert(2, "job_b".to_string(), 1);

  // Wait until nearly expired
  println!("Waiting 95ms (close to 100ms TTL)...");
  std::thread::sleep(Duration::from_millis(95));

  println!("Starting iteration (should complete in microseconds)...");

  // This should complete in microseconds
  let start = std::time::Instant::now();
  let mut iterations = 0;

  for (_k, _v) in cache.iter() {
    iterations += 1;

    // Safety check: if we've iterated more than expected, bail
    if iterations > 100 {
      panic!(
        "INFINITE LOOP DETECTED! Iterated {} times on 2 items",
        iterations
      );
    }

    // Safety check: if we've been iterating too long, bail
    if start.elapsed() > Duration::from_millis(500) {
      panic!(
        "Iterator taking too long! {} iterations in {:?}",
        iterations,
        start.elapsed()
      );
    }
  }

  let elapsed = start.elapsed();
  println!(
    "✓ Completed in {:?} with {} iterations",
    elapsed, iterations
  );

  assert!(
    iterations <= 2,
    "Should iterate at most 2 items, got {}",
    iterations
  );
  assert!(
    elapsed < Duration::from_millis(100),
    "Should complete quickly, took {:?}",
    elapsed
  );
}

// ======================================================================================
// Test 4: Production Scenario Simulation (1 Hour TTL)
// ======================================================================================

#[tokio::test]
async fn test_production_scenario_simulation() {
  println!("\n=== Test 4: Production Scenario (1 Hour TTL) ===");

  // Simulate TurnKeeper coordinator state
  let job_history = Arc::new(
    CacheBuilder::<u32, String>::new()
      .capacity(10_000)
      .time_to_live(Duration::from_secs(2)) // Use 2s instead of 3600s for testing
      .build()
      .unwrap(),
  );

  println!("Inserting 2 completed jobs with 2s TTL...");
  job_history.insert(1, "completed_job_1".to_string(), 1);
  job_history.insert(2, "completed_job_2".to_string(), 1);

  // Wait to get close to expiration boundary
  println!("Waiting 1.9s (close to TTL boundary)...");
  tokio::time::sleep(Duration::from_millis(1900)).await;

  println!("Calling list_all_jobs equivalent...");

  let iter_count = Arc::new(AtomicUsize::new(0));
  let count_clone = iter_count.clone();

  // Now call list_all_jobs equivalent
  let result = tokio::task::spawn_blocking(move || {
    let mut summaries = Vec::new();
    let start = std::time::Instant::now();
    let mut count = 0;

    for (_id, def) in job_history.iter() {
      count += 1;
      count_clone.store(count, Ordering::SeqCst);

      if count > 100 {
        println!("ERROR: {} iterations on 2 items!", count);
        return summaries;
      }

      if start.elapsed() > Duration::from_secs(2) {
        println!("ERROR: Iteration took {:?}", start.elapsed());
        return summaries;
      }

      summaries.push(def.clone());
    }

    println!("Completed: {} items in {:?}", count, start.elapsed());
    summaries
  });

  match tokio::time::timeout(Duration::from_secs(5), result).await {
    Ok(Ok(summaries)) => {
      println!("✓ Success: got {} summaries", summaries.len());
    }
    Ok(Err(e)) => panic!("Task panicked: {:?}", e),
    Err(_) => {
      let count = iter_count.load(Ordering::SeqCst);
      panic!(
        "REPRODUCED BUG: Iterator hung at TTL boundary after {} iterations!",
        count
      );
    }
  }
}

// ======================================================================================
// Test 5: Multiple Sequential Iterations (Warmup Effect)
// ======================================================================================

#[test]
fn test_multiple_sequential_iterations() {
  println!("\n=== Test 5: Multiple Sequential Iterations ===");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(200))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  cache.insert(1, "a".to_string(), 1);
  cache.insert(2, "b".to_string(), 1);
  cache.insert(3, "c".to_string(), 1);

  // Iterate multiple times as items approach expiration
  for attempt in 1..=5 {
    let delay_ms = 30 * attempt;
    println!(
      "Attempt {}: waiting {}ms, then iterating...",
      attempt, delay_ms
    );

    std::thread::sleep(Duration::from_millis(delay_ms));

    let start = std::time::Instant::now();
    let mut count = 0;

    for _ in cache.iter() {
      count += 1;
      if count > 50 {
        panic!(
          "Attempt {}: Infinite loop after {} iterations!",
          attempt, count
        );
      }
    }

    let elapsed = start.elapsed();
    println!("  → {} items in {:?}", count, elapsed);

    if elapsed > Duration::from_millis(500) {
      panic!("Attempt {}: Took too long: {:?}", attempt, elapsed);
    }
  }

  println!("✓ All attempts completed successfully");
}

// ======================================================================================
// Test 6: Empty Cache Edge Case
// ======================================================================================

#[test]
fn test_iter_empty_cache_with_ttl() {
  println!("\n=== Test 6: Empty Cache with TTL ===");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  // Insert and let them expire
  println!("Inserting 2 items, waiting for expiration...");
  cache.insert(1, "a".to_string(), 1);
  cache.insert(2, "b".to_string(), 1);

  std::thread::sleep(Duration::from_millis(150));

  println!("Items should be expired, iterating...");

  let start = std::time::Instant::now();
  let mut count = 0;

  for _ in cache.iter() {
    count += 1;
    if count > 10 {
      panic!("Infinite loop on empty/expired cache!");
    }
  }

  let elapsed = start.elapsed();
  println!("✓ Completed: {} items in {:?}", count, elapsed);

  assert!(count == 0, "Should iterate 0 items on expired cache");
  assert!(
    elapsed < Duration::from_millis(100),
    "Should complete quickly"
  );
}

// ======================================================================================
// Test 7: Stress Test - Rapid Iterations During Expiration Window
// ======================================================================================

#[tokio::test]
async fn test_rapid_iterations_during_expiration() {
  println!("\n=== Test 7: Rapid Iterations During Expiration ===");

  let cache = Arc::new(
    CacheBuilder::<u32, String>::new()
      .capacity(100)
      .time_to_live(Duration::from_millis(100))
      .timer_mode(TimerWheelMode::HighPrecisionShortLived)
      .build()
      .unwrap(),
  );

  cache.insert(1, "a".to_string(), 1);
  cache.insert(2, "b".to_string(), 1);

  // Wait until close to expiration
  tokio::time::sleep(Duration::from_millis(90)).await;

  println!("Starting rapid iterations during expiration window...");

  // Spawn multiple concurrent iterators
  let mut handles = vec![];

  for i in 0..5 {
    let cache_clone = cache.clone();
    let handle = tokio::task::spawn_blocking(move || {
      let start = std::time::Instant::now();
      let mut count = 0;

      for _ in cache_clone.iter() {
        count += 1;
        if count > 20 {
          return Err(format!("Iterator {}: infinite loop!", i));
        }
      }

      if start.elapsed() > Duration::from_secs(1) {
        return Err(format!("Iterator {}: took {:?}", i, start.elapsed()));
      }

      Ok((i, count))
    });

    handles.push(handle);
  }

  // Wait for all with timeout
  for handle in handles {
    match tokio::time::timeout(Duration::from_secs(3), handle).await {
      Ok(Ok(Ok((id, count)))) => {
        println!("  Iterator {} completed: {} items", id, count);
      }
      Ok(Ok(Err(e))) => panic!("{}", e),
      Ok(Err(e)) => panic!("Iterator panicked: {:?}", e),
      Err(_) => panic!("Iterator hung!"),
    }
  }

  println!("✓ All rapid iterations completed");
}

// ======================================================================================
// Test 8: Iterator State Corruption Check
// ======================================================================================

#[test]
fn test_iterator_state_consistency() {
  println!("\n=== Test 8: Iterator State Consistency ===");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  cache.insert(1, "a".to_string(), 1);
  cache.insert(2, "b".to_string(), 1);
  cache.insert(3, "c".to_string(), 1);

  std::thread::sleep(Duration::from_millis(90));

  println!("Creating iterator and partially consuming it...");

  let mut iter = cache.iter();
  let mut seen_keys = std::collections::HashSet::new();

  // Consume first item
  if let Some((key, _)) = iter.next() {
    seen_keys.insert(key);
    println!("  First item: key={}", key);
  }

  // Wait a bit (items might expire)
  std::thread::sleep(Duration::from_millis(20));

  println!("Consuming rest of iterator...");

  let start = std::time::Instant::now();
  let mut count = 1; // We already consumed one

  for (key, _) in iter {
    count += 1;

    // Check for duplicate keys (would indicate cycle)
    if seen_keys.contains(&key) {
      panic!("DUPLICATE KEY DETECTED: {} - ITERATOR CYCLE!", key);
    }
    seen_keys.insert(key);

    if count > 20 {
      panic!("Infinite loop detected after partial consumption!");
    }

    if start.elapsed() > Duration::from_millis(500) {
      panic!("Partial consumption took too long!");
    }
  }

  println!("✓ Iterator consumed {} unique items", count);
  assert!(count <= 3, "Should see at most 3 items");
}

// ======================================================================================
// Helper: Manual CPU monitoring test (requires manual observation)
// ======================================================================================

#[test]
#[ignore] // Run manually with: cargo test test_manual_cpu_observation -- --ignored --nocapture
fn test_manual_cpu_observation() {
  println!("\n=== Manual CPU Observation Test ===");
  println!("Monitor CPU usage while this test runs.");
  println!("If CPU spikes to 100% and test hangs, bug is reproduced!\n");

  let cache = CacheBuilder::<u32, String>::new()
    .capacity(100)
    .time_to_live(Duration::from_millis(100))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .build()
    .unwrap();

  cache.insert(1, "a".to_string(), 1);
  cache.insert(2, "b".to_string(), 1);

  println!("Waiting 95ms...");
  std::thread::sleep(Duration::from_millis(95));

  println!("Starting iteration NOW - check CPU!");
  println!("(Test will timeout after 10s if hung)\n");

  let start = std::time::Instant::now();
  let mut count = 0;

  for (_k, _v) in cache.iter() {
    count += 1;

    // Print every 1000 iterations to show progress
    if count % 1000 == 0 {
      println!("  ... {} iterations in {:?}", count, start.elapsed());
    }

    // Safety timeout
    if start.elapsed() > Duration::from_secs(10) {
      panic!("TIMEOUT: {} iterations in 10 seconds - HUNG!", count);
    }

    if count > 100000 {
      panic!("INFINITE LOOP: {} iterations!", count);
    }
  }

  println!("\n✓ Completed: {} items in {:?}", count, start.elapsed());
}
