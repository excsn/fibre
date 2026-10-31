#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use fibre_stats::{Batch, Histogram, Kind, Recorder, RecorderExt, Stats, StatsConfig, Tag, VecSink, collect, tags, value::NumberValue};

const THREADS: usize = 8;
const PER_THREAD: u64 = 100_000;

fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
  let (done, finished) = mpsc::channel();
  std::thread::spawn(move || {
    let _ = done.send(work());
  });
  finished.recv_timeout(Duration::from_secs(120)).expect("deadlocked or too slow")
}

fn churning_stats() -> Stats {
  Stats::new(StatsConfig {
    resolution: Duration::from_millis(1),
    flush_interval: Duration::from_millis(1),
    ..StatsConfig::default()
  })
}

fn on_threads(threads: usize, work: impl Fn(usize) + Send + Sync + 'static) {
  let work = Arc::new(work);
  let barrier = Arc::new(Barrier::new(threads));
  let handles: Vec<_> = (0..threads)
    .map(|thread| {
      let (work, barrier) = (work.clone(), barrier.clone());
      std::thread::spawn(move || {
        barrier.wait();
        work(thread);
      })
    })
    .collect();
  for handle in handles {
    handle.join().unwrap();
  }
}

fn counts(batches: &[Batch], name: &str) -> BTreeMap<Vec<Tag>, u64> {
  let mut sums = BTreeMap::new();
  for series in batches.iter().flat_map(|b| &b.series).filter(|s| &*s.name == name) {
    let sum: u64 = series.points.counts().expect("counts").iter().map(|(_, n)| n).sum();
    *sums.entry(series.tags.clone()).or_insert(0) += sum;
  }
  sums
}

#[test]
fn counters_lose_nothing_across_windows_and_flushes() {
  within_deadline(|| {
    let stats = churning_stats();
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| {
      for _ in 0..PER_THREAD {
        recorder.incr("shared", vec![]);
        recorder.add("own", tags!("thread" => thread.to_string()), 2);
      }
    });
    stats.stop();

    let batches = sink.take();
    assert!(batches.len() > 1, "every recording landed in {} batch", batches.len());
    assert_eq!(counts(&batches, "shared")[&vec![]], THREADS as u64 * PER_THREAD);
    let own = counts(&batches, "own");
    assert_eq!(own.len(), THREADS);
    assert!(own.values().all(|sum| *sum == 2 * PER_THREAD), "{own:?}");
    assert_eq!(stats.refused(), 0);
  });
}

#[test]
fn timers_keep_every_duration_across_windows_and_flushes() {
  within_deadline(|| {
    let stats = churning_stats();
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| {
      for i in 0..PER_THREAD {
        let micros = 1 + (i % 1000) + thread as u64;
        recorder.timer("latency", vec![], Duration::from_micros(micros));
      }
    });
    stats.stop();

    let mut merged = Histogram::new();
    for series in sink.take().iter().flat_map(|b| &b.series).filter(|s| &*s.name == "latency") {
      for (_, histogram) in series.points.histograms().unwrap() {
        merged.merge(histogram);
      }
    }
    let per_thread_base: u64 = (0..PER_THREAD).map(|i| 1 + i % 1000).sum();
    let thread_offsets: u64 = (0..THREADS as u64).map(|t| t * PER_THREAD).sum();
    assert_eq!(merged.count(), THREADS as u64 * PER_THREAD);
    assert_eq!(merged.sum(), Duration::from_micros(THREADS as u64 * per_thread_base + thread_offsets));
    assert_eq!(merged.min(), Duration::from_micros(1));
    assert_eq!(merged.max(), Duration::from_micros(1000 + THREADS as u64 - 1));
  });
}

#[test]
fn a_gauge_with_one_writer_keeps_its_order_and_final_value() {
  within_deadline(|| {
    let stats = churning_stats();
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| {
      for i in 1..=PER_THREAD {
        recorder.gauge("level", tags!("thread" => thread.to_string()), i);
      }
    });
    stats.stop();

    let mut points: BTreeMap<Vec<Tag>, Vec<(u64, f64)>> = BTreeMap::new();
    for series in sink.take().iter().flat_map(|b| &b.series).filter(|s| &*s.name == "level") {
      let entry = points.entry(series.tags.clone()).or_default();
      entry.extend(series.points.numbers().unwrap().iter().map(|(at, v)| (*at, v.to_f64())));
    }
    assert_eq!(points.len(), THREADS);
    for (tags, mut series) in points {
      series.sort_by_key(|(at, _)| *at);
      assert!(series.windows(2).all(|w| w[0].1 <= w[1].1), "{tags:?} went backwards");
      assert_eq!(series.last().unwrap().1, PER_THREAD as f64, "{tags:?}");
    }
  });
}

#[test]
fn running_totals_from_many_threads_become_exact_deltas() {
  within_deadline(|| {
    let stats = churning_stats();
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| {
      for i in 1..=PER_THREAD {
        recorder.total("bytes".into(), tags!("thread" => thread.to_string()), 1000 + i);
      }
    });
    stats.stop();

    let sums = counts(&sink.take(), "bytes");
    assert_eq!(sums.len(), THREADS);
    assert!(sums.values().all(|sum| *sum == PER_THREAD - 1), "{sums:?}");
  });
}

#[test]
fn racing_first_recordings_of_two_kinds_settle_on_one_and_count_the_rest() {
  within_deadline(|| {
    for round in 0..200 {
      let stats = Stats::new(StatsConfig::default());
      let recorder = stats.clone();
      const EACH: u64 = 200;
      on_threads(4, move |thread| {
        for _ in 0..EACH {
          if thread % 2 == 0 {
            recorder.incr("contested", vec![]);
          } else {
            recorder.gauge("contested", vec![], 1u64);
          }
        }
      });
      let batch = stats.drain(true);
      assert_eq!(batch.series.len(), 1, "round {round}");
      assert_eq!(stats.refused(), 2 * EACH, "round {round}");
      if batch.series[0].kind == Kind::Counter {
        assert_eq!(batch.series[0].points.counts().unwrap().iter().map(|(_, n)| n).sum::<u64>(), 2 * EACH);
      }
    }
  });
}

#[test]
fn context_and_common_tags_reach_every_series_under_load() {
  within_deadline(|| {
    let stats = Stats::new(StatsConfig {
      common_tags: tags!("app" => "shop"),
      resolution: Duration::from_millis(1),
      flush_interval: Duration::from_millis(1),
      ..StatsConfig::default()
    });
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| {
      let context = recorder.context(tags!("thread" => thread.to_string()));
      for _ in 0..PER_THREAD {
        context.incr("work", vec![]);
      }
    });
    stats.stop();

    let batches = sink.take();
    assert!(batches.iter().all(|b| b.common_tags == tags!("app" => "shop")));
    let sums = counts(&batches, "work");
    assert_eq!(sums.len(), THREADS);
    assert!(sums.values().all(|sum| *sum == PER_THREAD), "{sums:?}");
  });
}

#[test]
fn collectors_can_be_added_run_and_removed_while_others_record() {
  within_deadline(|| {
    let stats = churning_stats();
    let sink = Arc::new(VecSink::new());
    stats.start(sink.clone());
    let recorder = stats.clone();
    on_threads(THREADS, move |thread| match thread % 4 {
      0 => {
        for _ in 0..2_000 {
          let id = recorder.add_collector(Duration::from_micros(50), collect::gauge("polled", vec![], || NumberValue::count(1)));
          assert!(recorder.remove_collector(id));
        }
      }
      1 => {
        for _ in 0..2_000 {
          recorder.collect_now();
          recorder.collect_due();
        }
      }
      _ => {
        for _ in 0..PER_THREAD {
          recorder.incr("work", vec![]);
        }
      }
    });
    stats.stop();
    let expected = THREADS as u64 / 2 * PER_THREAD;
    assert_eq!(counts(&sink.take(), "work")[&vec![]], expected);
  });
}

#[test]
fn recordings_racing_stop_reach_the_sink_when_the_last_handle_drops() {
  within_deadline(|| {
    for _ in 0..20 {
      let stats = churning_stats();
      let sink = Arc::new(VecSink::new());
      stats.start(sink.clone());
      let recorded = Arc::new(AtomicU64::new(0));
      let running = Arc::new(AtomicBool::new(true));
      let writers: Vec<_> = (0..4)
        .map(|_| {
          let (stats, recorded, running) = (stats.clone(), recorded.clone(), running.clone());
          std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
              stats.incr("work", vec![]);
              recorded.fetch_add(1, Ordering::Relaxed);
            }
          })
        })
        .collect();
      std::thread::sleep(Duration::from_millis(5));
      stats.stop();
      std::thread::sleep(Duration::from_millis(5));
      running.store(false, Ordering::Relaxed);
      for writer in writers {
        writer.join().unwrap();
      }
      drop(stats);
      let total = counts(&sink.take(), "work")[&vec![]];
      assert_eq!(total, recorded.load(Ordering::Relaxed));
    }
  });
}

#[test]
fn dropping_the_last_handle_while_background_threads_run_flushes_and_ends() {
  within_deadline(|| {
    for _ in 0..200 {
      let stats = churning_stats();
      let sink = Arc::new(VecSink::new());
      stats.start(sink.clone());
      stats.add_collector(Duration::from_micros(100), collect::gauge("polled", vec![], || NumberValue::count(1)));
      let recorder = stats.clone();
      drop(stats);
      on_threads(4, {
        let recorder = recorder.clone();
        move |_| {
          for _ in 0..200 {
            recorder.incr("work", vec![]);
          }
        }
      });
      drop(recorder);
      let deadline = std::time::Instant::now() + Duration::from_secs(5);
      let mut batches = Vec::new();
      loop {
        batches.extend(sink.take());
        if counts(&batches, "work").get(&vec![]) == Some(&800) {
          break;
        }
        assert!(std::time::Instant::now() < deadline, "{:?}", counts(&batches, "work"));
        std::thread::yield_now();
      }
    }
  });
}

#[cfg(feature = "high_res")]
mod high_res {
  use super::*;
  use fibre_stats::metric;

  metric! {
    /// Kept exactly as well as aggregated.
    #[high_res("1ms")]
    pub static FRAME: Timer = "stress.frame";
  }

  #[test]
  fn kept_and_dropped_samples_add_up_to_what_was_recorded() {
    within_deadline(|| {
      let stats = Stats::new(StatsConfig {
        resolution: Duration::from_millis(1),
        flush_interval: Duration::from_millis(1),
        high_res_buffer: 64,
        ..StatsConfig::default()
      });
      let sink = Arc::new(VecSink::new());
      stats.start(sink.clone());
      let recorder = stats.clone();
      on_threads(THREADS, move |_| {
        for _ in 0..PER_THREAD {
          FRAME.record_to(&recorder, Duration::from_micros(5), tags!());
        }
      });
      stats.stop();

      let (mut kept, mut dropped, mut aggregated) = (0u64, 0u64, 0u64);
      for batch in sink.take() {
        dropped += batch.dropped.iter().map(|d| d.count).sum::<u64>();
        for series in batch.series.iter().filter(|s| &*s.name == "stress.frame") {
          if let Some(samples) = series.points.samples() {
            kept += samples.len() as u64;
          }
          if let Some(histograms) = series.points.histograms() {
            aggregated += histograms.iter().map(|(_, h)| h.count()).sum::<u64>();
          }
        }
      }
      let recorded = THREADS as u64 * PER_THREAD;
      assert_eq!(aggregated, recorded);
      assert_eq!(kept + dropped, recorded, "kept {kept} dropped {dropped}");
      assert!(dropped > 0, "the buffer never filled, so the drop path did not run");
    });
  }
}
