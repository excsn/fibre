# Memory-model readings

Handshakes whose correctness rests on sequential consistency. Shuttle treats every atomic as SeqCst. Loom 0.7 treats SeqCst loads, stores and RMWs as AcqRel (its README, "Unsupported features"), modelling SeqCst only through `fence(SeqCst)`. Miri's weak-memory emulation samples these outcomes by seed rather than exhaustively. Each entry names the two sides, the clauses the argument uses and its status. Rust atomics follow the C++20 memory model.

Clauses used, C++20 [atomics.order]:

- **p3.** A is coherence-ordered before B (both on M) if A is a modification and B reads the value A stored; or A precedes B in the modification order of M; or A and B are not the same RMW and A reads the value stored by some X that precedes B in the modification order; or transitively.
- **p4.** There is a single total order S on all `seq_cst` operations, fences included. If A and B are `seq_cst` operations and A strongly happens before B, A precedes B in S. For every pair A, B on one object where A is coherence-ordered before B:
  - **(4.1)** if A and B are both `seq_cst` operations, A precedes B in S;
  - **(4.2)** if A is a `seq_cst` operation and B happens before a `seq_cst` fence Y, A precedes Y in S;
  - **(4.3)** if a `seq_cst` fence X happens before A and B is a `seq_cst` operation, X precedes B in S;
  - **(4.4)** if a `seq_cst` fence X happens before A and B happens before a `seq_cst` fence Y, X precedes Y in S.

## bounded MPSC: drain versus parked or leaving sender

Sides:

- **Consumer** (`deq_once_locked` / `deq_run_locked`, then `post_drain`): store Wd of `drained` (Release) under the head lock, head unlock U (SeqCst swap on `locked`), then SeqCst loads Rw of `awake` and the send-waiter counts.
- **Parking sender** (`register_sync_send` / `register_async_send`, then the recheck): store Ww of the waiter count (Release) under the registry lock, `fence(SeqCst)` Fp, then a load Rd of `progress` / `drained`.
- **Leaving sender** (`awake_release`): `awake.fetch_sub` Ww (AcqRel), `fence(SeqCst)` Fp, then a load Rd of `g_tail` / `drained`.

Required: never both sides stale. If Rw reads the old count or the old `awake`, Rd must read the new `drained`.

Stale-stale with the code as it is:

1. Rw reads a value that precedes Ww in modification order, so Rw is coherence-ordered before Ww (p3). Rw is `seq_cst` and Ww is sequenced before Fp, so Rw precedes Fp in S (4.2).
2. Rd reads a value that precedes Wd, so Rd is coherence-ordered before Wd. Fp happens before Rd. For this pair to order anything in S, Wd must be a `seq_cst` operation (4.3) or happen before a `seq_cst` fence (4.4). Wd is a Release store; U is an RMW on another object and Rw is a load, neither a fence.
3. S = U, Rw, Fp satisfies every clause. The stale-stale outcome is allowed by C++20.

With Wd SeqCst: Fp happens before Rd and Wd is a `seq_cst` operation, so Fp precedes Wd in S (4.3). Wd is sequenced before Rw and both are `seq_cst`, so Wd precedes Rw in S (p4, first constraint). With step 1 that is Fp, Wd, Rw, Fp: a cycle, so the outcome is forbidden.

With a `fence(SeqCst)` Fc between U and Rw instead: Wd happens before Fc, so Fp precedes Fc (4.4 with Rd, Wd). Fc happens before Rw and Ww happens before Fp, so Fc precedes Fp (4.4 with Rw, Ww). A cycle again.

The stores that precede `post_drain` are the per-item store in `deq_once_locked` and the final store in `deq_run_locked`. The stores in `publish_progress` and `early_wake` reach `gated_notify`, whose first statement is `fence(SeqCst)`, so 4.4 already covers them.

Codegen (rustc -O, the consumer's store, unlock and load):

| sequence | aarch64-apple-darwin | x86_64 |
|---|---|---|
| Wd Release (current) | `stlr; swpalb; ldar` | `mov; xchg; mov` |
| Wd SeqCst | `stlr; swpalb; ldar` | `xchg; xchg; mov` |
| Wd Release, `fence(SeqCst)` before Rw | `stlr; swpalb; dmb ish; ldar` | `mov; xchg; lock or; mov` |

On aarch64 an `ldar` cannot be satisfied before an earlier `stlr` is visible. On x86 the unlock's `xchg` drains the store buffer. The stale-stale outcome cannot occur on either machine today. It is a language-level gap, not an observed failure.

Checking:

- **Miri** (`miri/tests/mpsc_bounded.rs`, `async_send_parked_on_full_sync_receiver_resumes_on_single_drain`) covers it. Miri's weak-memory emulation gives SeqCst accesses their C++20 meaning. With the Wd stores reverted to Release the test deadlocked on 52 of 256 seeds; with weak-memory emulation disabled the same mutant passed 64 of 64; with the fix it passed 256 of 256. At 64 seeds a regression is missed with probability about 0.8^64.
- **Shuttle** (`single_drain_releases_blocked_async_sender_pct`) covers the schedule logic of the same scenario. It treats every atomic as SeqCst, so it cannot reach the stale-stale outcome.
- **Loom** reaches the outcome but models SeqCst stores as AcqRel, so it reports it with the fix in place. The model lives in `loom/tests/mpsc_bounded_advisory.rs`, which `scripts/loom.sh` runs without counting its result.

Status: fixed. Both Wd stores are SeqCst (`deq_once_locked`, `deq_run_locked`). On aarch64 the consumer compiles to identical machine code; on x86 each drain's store becomes an `xchg`. The fence variant cost 16-27% at cap 128 in the lab and was not taken.
