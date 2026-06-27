use clap::{Parser, ValueEnum};

mod runners;
mod watchdog;

use runners::BenchConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Channel {
  /// mpmc_v3 bounded (Vyukov ring + tombstone handoff); rendezvous/unbounded delegate to mpmc-v2
  #[value(name = "mpmc-v3")]
  MpmcV3,
  /// mpmc_v2 (lock-based bounded/unbounded + rendezvous)
  #[value(name = "mpmc-v2")]
  MpmcV2,
  /// multi-producer single-consumer (single consumer enforced)
  Mpsc,
  /// single-producer multi-consumer broadcast (single producer enforced, T: Clone)
  Spmc,
  /// single-producer single-consumer bounded (1 producer + 1 consumer enforced)
  Spsc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
  Sync,
  Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Flavor {
  Bounded,
  Unbounded,
  Rendezvous,
}

#[derive(Parser, Debug)]
#[command(name = "fibre_bench", about = "Fibre channel throughput benchmark")]
struct Args {
  #[arg(short = 'c', long, default_value = "mpmc-v3", help = "Channel family")]
  channel: Channel,

  #[arg(short = 'm', long, default_value = "sync", help = "Sync or async mode")]
  mode: Mode,

  #[arg(
    short = 'f',
    long,
    default_value = "bounded",
    help = "Channel flavor (bounded/unbounded/rendezvous)"
  )]
  flavor: Flavor,

  #[arg(short = 'p', long, default_value_t = 1, help = "Number of producers")]
  producers: usize,

  #[arg(short = 'n', long, default_value_t = 1, help = "Number of consumers")]
  consumers: usize,

  #[arg(short = 'C', long, default_value_t = 128, help = "Channel capacity (bounded only)")]
  capacity: usize,

  #[arg(short = 'i', long, default_value_t = 1_000_000, help = "Total items to send")]
  items: usize,

  #[arg(long, default_value_t = 500, help = "Watchdog poll interval (ms)")]
  stall_ms: u64,

  #[arg(long, default_value_t = 4, help = "Stall ticks before abort")]
  stall_count: usize,
}

fn channel_name(c: Channel) -> &'static str {
  match c {
    Channel::MpmcV3 => "mpmc-v3",
    Channel::MpmcV2 => "mpmc-v2",
    Channel::Mpsc => "mpsc",
    Channel::Spmc => "spmc",
    Channel::Spsc => "spsc",
  }
}

fn mode_name(m: Mode) -> &'static str {
  match m {
    Mode::Sync => "sync",
    Mode::Async => "async",
  }
}

fn flavor_name(f: Flavor) -> &'static str {
  match f {
    Flavor::Bounded => "bounded",
    Flavor::Unbounded => "unbounded",
    Flavor::Rendezvous => "rendezvous",
  }
}

fn main() {
  let args = Args::parse();

  // Cardinality constraints
  match args.channel {
    Channel::Spsc => {
      if args.producers != 1 {
        eprintln!("error: spsc requires --producers 1 (got {})", args.producers);
        std::process::exit(1);
      }
      if args.consumers != 1 {
        eprintln!("error: spsc requires --consumers 1 (got {})", args.consumers);
        std::process::exit(1);
      }
      if !matches!(args.flavor, Flavor::Bounded) {
        eprintln!("error: spsc only supports bounded flavor");
        std::process::exit(1);
      }
    }
    Channel::Mpsc => {
      if args.consumers != 1 {
        eprintln!("error: mpsc requires --consumers 1 (got {})", args.consumers);
        std::process::exit(1);
      }
      if matches!(args.flavor, Flavor::Rendezvous) {
        eprintln!("error: mpsc does not support rendezvous flavor");
        std::process::exit(1);
      }
    }
    Channel::Spmc => {
      if args.producers != 1 {
        eprintln!("error: spmc requires --producers 1 (got {})", args.producers);
        std::process::exit(1);
      }
      if !matches!(args.flavor, Flavor::Bounded) {
        eprintln!("error: spmc only supports bounded flavor");
        std::process::exit(1);
      }
    }
    _ => {}
  }

  let is_spmc = matches!(args.channel, Channel::Spmc);

  println!(
    "Channel: {} | Mode: {} | Flavor: {} | Cap: {} | Producers: {} | Consumers: {} | Items: {}",
    channel_name(args.channel),
    mode_name(args.mode),
    flavor_name(args.flavor),
    args.capacity,
    args.producers,
    args.consumers,
    args.items,
  );

  let cfg = BenchConfig {
    producers: args.producers,
    consumers: args.consumers,
    capacity: args.capacity,
    items: args.items,
    stall_ms: args.stall_ms,
    stall_count: args.stall_count,
  };

  let result = match (args.channel, args.mode, args.flavor) {
    (Channel::MpmcV3, Mode::Sync, Flavor::Bounded) => runners::mpmc_v3::run_sync(&cfg),
    (Channel::MpmcV3, Mode::Async, Flavor::Bounded) => runners::mpmc_v3::run_async(&cfg),
    (Channel::MpmcV3, mode, flavor) | (Channel::MpmcV2, mode, flavor) => match mode {
      Mode::Sync => runners::mpmc_v2::run_sync(&cfg, &flavor),
      Mode::Async => runners::mpmc_v2::run_async(&cfg, &flavor),
    },
    (Channel::Mpsc, Mode::Sync, flavor) => runners::mpsc::run_sync(&cfg, &flavor),
    (Channel::Mpsc, Mode::Async, flavor) => runners::mpsc::run_async(&cfg, &flavor),
    (Channel::Spmc, Mode::Sync, _) => runners::spmc::run_sync(&cfg),
    (Channel::Spmc, Mode::Async, _) => runners::spmc::run_async(&cfg),
    (Channel::Spsc, Mode::Sync, _) => runners::spsc::run_sync(&cfg),
    (Channel::Spsc, Mode::Async, _) => runners::spsc::run_async(&cfg),
  };

  let secs = result.duration.as_secs_f64();
  let throughput_items = if secs > 0.0 {
    // For spmc, sent = N but received = N * consumers (each consumer sees every item).
    // Report dispatch throughput (sent/sec) so it's comparable to other channel types.
    if is_spmc { result.sent } else { result.received }
  } else {
    0
  };
  let throughput_m = if secs > 0.0 { throughput_items as f64 / secs / 1_000_000.0 } else { f64::INFINITY };

  if is_spmc {
    println!(
      "Completed in {:.1?} — sent={} received={} ({}x broadcast) — {:.2}M dispatched/sec",
      result.duration,
      result.sent,
      result.received,
      args.consumers,
      throughput_m,
    );
  } else {
    println!(
      "Completed in {:.1?} — sent={} received={} — {:.2}M items/sec",
      result.duration, result.sent, result.received, throughput_m,
    );
  }
  fibre::telemetry::print_stall_report();
}
