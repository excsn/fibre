# channels/scripts - benchmark wrapper

The validation tiers (stress, shuttle, loom, miri, bolero) and their runners live in `channels/verify/`; see `../verify/GUIDE.md`.

## `bench.sh <bench-name> [criterion-filter] [extra criterion args...]`

Criterion filter wrapper. It `cd`s to the workspace root, so it works from anywhere. **Baseline caveat:** an isolated filtered run overwrites only the baselines it touches, so a later run can report "no change" against a stale/partial baseline - for a real A/B, save a clean baseline first:

```sh
channels/scripts/bench.sh mpsc_bounded 'MpscBoundedSync/Cap-128'
# clean A/B vs a commit:
git checkout <baseline-commit>
channels/scripts/bench.sh mpsc_bounded '' --save-baseline pre
git checkout -
channels/scripts/bench.sh mpsc_bounded '' --baseline pre
```

For profiling (flamegraph, single long run) use `fibre_bench` instead - see `../bench/GUIDE.md`.
