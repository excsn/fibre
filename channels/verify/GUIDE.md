# channels/verify - fibre's validation tiers

Unit and regression tests live with the code (`channels/src/**` test modules and `channels/tests/`). Everything that needs an external tool, a special build or a long run lives here, one crate per tier, all members of the root workspace. Each tier catches a class of defect the others cannot, so a change to a channel protocol runs every tier that covers that channel.

| Tier | Crate | Runner | Catches |
| :--- | :--- | :--- | :--- |
| 1 | `channels` test modules | `cargo test -p fibre` | behaviour and regressions |
| 2 | `stress` | `scripts/stress.sh [file]` | anything the real machine does, at scale: lost wakeups, hangs, throughput-dependent races |
| 3 | `shuttle` | `scripts/shuttle.sh [file]` | interleaving and liveness bugs at realistic scale, via randomized and PCT schedules; every atomic is sequentially consistent |
| 4 | `loom` | `scripts/loom.sh [file] [filter] [preemptions]` | exhaustive small models of acquire/release and fence protocols |
| 5 | `miri` | `scripts/miri.sh <file> [seeds] [filter]` | undefined behaviour, data races, use-after-free, aliasing |
| 6 | `MODEL.md` | reading | sequentially consistent pairings no checker here models |
| 7 | `bolero` | `scripts/bolero.sh [file]` | operation-sequence properties against a model queue |

## What each tier cannot see

- **Stress** only finds what the OS scheduler happens to produce. A pass is evidence, not proof. Vary the worker count: the bounded MPSC release-after-send hang only reproduced on 2 tokio workers.
- **Shuttle** treats every atomic as sequentially consistent, so a Release where SeqCst is required looks correct to it.
- **Loom** 0.7 models sequential consistency only through `fence(SeqCst)`; SeqCst loads and stores are treated as acquire/release. A handshake built on SeqCst atomics can show a false deadlock there, so its verdict on such a pairing is not evidence either way. Keep models tiny: capacity 1-2, 1-2 items per thread, at most 2 spawned threads, no timeouts or sleeps (the mocked primitives panic on them).
- **Miri** samples schedules; run many seeds (`scripts/miri.sh <file> 0..64`) and both the isolated test and the whole file. It is slow: every operation is interpreted, the race detector runs and the harness uses every core, so a wide seed range can take an hour at full CPU.
- **MODEL.md** is only as good as the reading; each entry names the rules it rests on.

## Instrumented primitives

Channels take their atomics, locks, threads and parking from `channels/src/internal/sync.rs`. Normal builds re-export std and parking_lot. Under `--cfg loom` they are loom's types. Under `--cfg shuttle` they are shuttle's. Anything blocking must come from the facade, or a model run blocks for real and hangs.

## Running tiers

One expensive tier at a time. Miri, loom, shuttle, stress and benchmarks all saturate the machine; running two together slows both and makes timing-dependent failures appear and disappear.
