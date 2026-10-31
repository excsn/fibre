use std::sync::Arc;
use std::time::Duration;

use fibre_stats::{
  EnumState, Kind, Stats, StatsConfig, StateValue, Unit, VecSink, metric, registry, state_enum, tags,
};

metric! {
  /// Requests answered.
  /// One per response.
  #[tags("status", "route")]
  pub static REQUESTS: Counter = "test.http.requests";

  /// Time to answer one request.
  #[tags("route")]
  pub static LATENCY: Timer = "test.http.latency";

  /// Connections open.
  pub static CONNECTIONS: Gauge = "test.http.connections";

  /// Where the server is in its life.
  pub static STATUS: State = "test.server.status";

  #[unit("rpm")]
  static FAN: Gauge = "test.fan.speed";

  #[unit(bytes)]
  #[high_res("1ms")]
  pub static FRAME: Timer = "test.frame";
}

state_enum! {
  /// Where a server is in its life.
  pub enum ServerStatus {
    Starting = "starting",
    Serving = "serving",
    Draining = "draining",
  }
}

#[test]
fn a_registered_definition_appears_once() {
  assert_eq!(registry().filter(|m| m.name == "test.http.requests").count(), 1);
}

#[test]
fn definitions_are_in_the_registry_with_their_facts() {
  let requests = registry().find(|m| m.name == "test.http.requests").unwrap();
  assert_eq!(requests.kind, Kind::Counter);
  assert_eq!(requests.unit, Unit::Count);
  assert_eq!(requests.description_text(), "Requests answered.\nOne per response.");
  assert_eq!(requests.tags, &["status", "route"]);

  let latency = fibre_stats::find("test.http.latency").unwrap();
  assert_eq!(latency.kind, Kind::Timer);
  assert_eq!(latency.unit, Unit::Seconds);

  let fan = fibre_stats::find("test.fan.speed").unwrap();
  assert_eq!(fan.unit, Unit::Custom("rpm"));
  assert!(fan.description.is_empty());

  let frame = fibre_stats::find("test.frame").unwrap();
  assert_eq!(frame.unit, Unit::Bytes);
  assert_eq!(frame.resolution, Some(Duration::from_millis(1)));

  assert!(std::ptr::eq(fibre_stats::find("test.server.status").unwrap(), &STATUS.metric));
}

#[test]
fn handles_record_into_a_stats() {
  let stats = Stats::new(StatsConfig::default());
  REQUESTS.add_to(&stats, 2, tags!("status" => "ok", "route" => "/"));
  REQUESTS.add_to(&stats, 1, tags!("status" => "ok", "route" => "/"));
  LATENCY.time_to(&stats, tags!("route" => "/"), || {});
  {
    let guard = LATENCY.guard_to(&stats, tags!("route" => "/"));
    assert!(guard.elapsed() < Duration::from_secs(1));
  }
  LATENCY.guard_to(&stats, tags!("route" => "/x")).discard();
  CONNECTIONS.set_to(&stats, 4u32, tags!());
  STATUS.set_to(&stats, ServerStatus::Serving, tags!());

  let batch = stats.drain(true);
  let requests = batch
    .series("test.http.requests", &tags!("route" => "/", "status" => "ok"))
    .unwrap();
  assert_eq!(requests.points.counts().unwrap()[0].1, 3);

  let latency = batch.series("test.http.latency", &tags!("route" => "/")).unwrap();
  assert_eq!(latency.points.histograms().unwrap()[0].1.count(), 2);
  assert!(batch.series("test.http.latency", &tags!("route" => "/x")).is_none());

  let status = batch.series("test.server.status", &tags!()).unwrap();
  assert_eq!(
    status.points.states().unwrap()[0].1,
    StateValue::label("ServerStatus", "serving")
  );
  assert_eq!(ServerStatus::Draining.label(), "draining");
}

#[test]
fn a_sink_receives_the_batch_on_stop() {
  let stats = Stats::new(StatsConfig {
    common_tags: tags!("env" => "test"),
    ..StatsConfig::default()
  });
  let sink = Arc::new(VecSink::new());
  stats.start(sink.clone());
  CONNECTIONS.set_to(&stats, 1u8, tags!());
  stats.stop();
  let batches = sink.take();
  assert_eq!(batches.len(), 1);
  assert_eq!(batches[0].common_tags, tags!("env" => "test"));
  assert_eq!(batches[0].series[0].name, "test.http.connections");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "does not declare tag")]
fn an_undeclared_tag_panics_in_debug_builds() {
  let stats = Stats::new(StatsConfig::default());
  CONNECTIONS.set_to(&stats, 1u8, tags!("host" => "a"));
}
