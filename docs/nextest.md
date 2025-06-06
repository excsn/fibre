# Debugging Concurrency in `fibre` with `cargo-nextest` and Sanitizers

`cargo-nextest` is a powerful test runner that can significantly aid in debugging complex concurrency issues within the `fibre` library, such as intermittent hangs or data races. This guide outlines how to install `nextest` and use it effectively with ThreadSanitizer (TSan) to diagnose problems in `fibre`'s channel implementations.

## Why `cargo-nextest` for `fibre`?

*   **Per-Test Timeouts:** Crucial for automatically terminating tests that might hang due to deadlocks in `fibre`'s synchronization logic (e.g., in `spmc`, `mpmc`).
*   **Better Performance for Large Test Suites:** `fibre` has many test configurations; `nextest` can run them more efficiently.
*   **Improved UI:** Clearer output for test successes and failures.

## Installation

`cargo-nextest` is installed via `cargo install`.

### macOS

If you use [Homebrew](https://brew.sh/):
```bash
brew install cargo-nextest
```
Alternatively:
```bash
cargo install cargo-nextest
```

### Linux

```bash
cargo install cargo-nextest
```
Ensure `~/.cargo/bin` is in your `PATH`. Add to your shell's config (e.g., `~/.bashrc`, `~/.zshrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```
Then reload your shell configuration.

## Basic Usage with `fibre` Tests

To run all tests within the `fibre` workspace:
```bash
cargo nextest run
```

To run a specific test function, for example, one of the SPMC deadlock reproduction tests located in `tests/spmc_repro.rs` within the `spmc_deadlock_tests` module:
```bash
cargo nextest run spmc_deadlock_tests::looped_repro_spmc_sync_hang_4c_1cap
```
Or, more generally, if your test file is `tests/some_channel_test.rs`:
```bash
cargo nextest run --test some_channel_test -- your_test_name_filter
```

To see `println!` or `fibre::telemetry` output immediately:
```bash
cargo nextest run --nocapture-output immediate
```

## Using ThreadSanitizer (TSan) with `fibre` Tests

TSan helps detect data races in `fibre`'s `unsafe` code or complex atomic interactions (e.g., in `mpmc/core.rs`, `spmc/ring_buffer.rs`, `mpsc/lockfree.rs`).

**Prerequisites for TSan:**
*   **Rust Nightly:** `rustup toolchain install nightly`
*   **Clang Runtime Libraries:**
    *   **macOS:** Xcode Command Line Tools (`xcode-select --install`).
    *   **Linux:** Install `clang`, `libclang-dev`, and TSan runtimes (e.g., `libtsan0` on Debian/Ubuntu).

**Running `nextest` with TSan for `fibre`:**

Enable the `fibre_telemetry` feature if you want telemetry output during TSan runs.

**Example: Testing `fibre`'s SPMC sync logic on macOS (Apple Silicon)**
```bash
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly nextest run \
  spmc_deadlock_tests::looped_repro_spmc_sync_hang_4c_1cap \
  --target aarch64-apple-darwin \
  --features fibre_telemetry \
  --nocapture-output immediate
```

**Example: Testing `fibre`'s MPMC sync logic on Linux (x86_64)**
```bash
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly nextest run \
  mpmc_sync_tests::some_mpmc_test \
  --target x86_64-unknown-linux-gnu \
  --features fibre_telemetry \
  --nocapture-output immediate
```

**Key Command Parts for `fibre` + TSan + Nextest:**
*   `RUSTFLAGS="-Z sanitizer=thread"`: Enables TSan.
*   `cargo +nightly nextest run`: Uses `nextest` with the nightly toolchain.
*   `your_fibre_test_path`: e.g., `spmc_deadlock_tests::looped_repro_spmc_sync_hang_4c_1cap`.
*   `--target <your-target-triple>`: Essential for TSan.
*   `--features fibre_telemetry`: To enable `fibre`'s internal telemetry for detailed logs if the test includes telemetry calls.
*   `--nocapture-output immediate`: Critical for seeing TSan warnings and telemetry logs as they happen.

**Interpreting TSan Output for `fibre`:**
*   TSan warnings will point to specific lines in `fibre`'s source files (e.g., `src/spmc/ring_buffer.rs`, `src/mpmc/core.rs`).
*   Analyze the read/write operations and the threads involved.
*   Races on fields within `SpmcShared`, `MpscShared`, `MpmcShared`, or individual `Slot`s are direct indicators of bugs in `fibre`.
*   Races reported during teardown (e.g., involving `Arc::drop_slow` and `AtomicWaker::wake`) might be TSan sensitivities or very subtle drop-order issues. Prioritize races that occur during the main operational logic of the channels first.

**Strategy for Debugging `fibre` Deadlocks/Races with `nextest`:**

1.  **Isolate the Problematic Test:** Use `nextest` to run only the specific `fibre` test configuration (e.g., a specific SPMC consumer/capacity combination) that hangs or is suspect.
    ```bash
    # Example: If your looped SPMC test hangs
    cargo nextest run spmc_deadlock_tests::looped_repro_spmc_sync_hang_4c_1cap --nocapture-output immediate --features fibre_telemetry
    ```
2.  **Enable Detailed Telemetry:** Ensure your `fibre` code (especially the suspected module like `spmc::ring_buffer`) is heavily instrumented with `fibre::telemetry::log_event` calls around parking, waking, and state changes.
3.  **Run with `nextest` (No TSan First):** See if the test hits your internal timeout in `run_spmc_iteration`. The telemetry report printed by your test logic upon timeout will be the primary source for understanding the deadlock state.
4.  **Run with `nextest` and TSan:** If the test passes without TSan but you suspect subtle races, or if the non-TSan run is inconclusive, use TSan.
    ```bash
    RUSTFLAGS="-Z sanitizer=thread" cargo +nightly nextest run \
      spmc_deadlock_tests::looped_repro_spmc_sync_hang_4c_1cap \
      --target <your-target> \
      --features fibre_telemetry \
      --nocapture-output immediate
    ```
    Analyze any TSan reports. If TSan reports a race in `fibre` code before a hang, that's a prime suspect. If the `fibre` code seems clean to TSan but the test still hangs (and your internal timeout triggers, printing telemetry), then it's a pure logical deadlock.

By combining `nextest`'s test management with `fibre`'s own telemetry and TSan, you have a robust toolkit for diagnosing these challenging concurrent issues.