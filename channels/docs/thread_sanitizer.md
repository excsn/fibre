## Sanitizing Rust Code with Thread Sanitizer (TSan)

A concise guide to enable and run Thread Sanitizer (TSan) for Rust projects on macOS using the nightly toolchain, without altering your default setup.

---

### 1. Prerequisites

* **Rustup**: Ensure `rustup` is installed and up to date.
* **Nightly Toolchain**: Required for the `-Z sanitizer` flag.
* **Xcode Command‑Line Tools**: Provides necessary compilers and sanitizers on macOS.

---

### 2. Install Nightly (if needed)

Use Rustup to add the nightly toolchain alongside your stable default:

```bash
rustup toolchain install nightly
```

No changes to your default channel are made.

---

### 3. Running Tests with TSan

1. **Set the sanitizer flag:**

   ```bash
   export RUSTFLAGS="-Z sanitizer=thread"
   ```
2. **Invoke tests on nightly:**

   ```bash
   cargo +nightly test --target <target-triple>
   ```

   Replace `<target-triple>` with your architecture, for example `aarch64-apple-darwin` (M1/M2) or `x86_64-apple-darwin` (Intel).

> **Tip:** Combine steps into one line if you prefer:
>
> ```bash
> RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test --target aarch64-apple-darwin
> ```

---

### 4. Running Benchmarks with TSan

To measure performance under TSan, use `cargo bench` similarly:

* **Single benchmark case:**

  ```bash
  RUSTFLAGS="-Z sanitizer=thread" \
  cargo +nightly bench --bench spmc_sync --target <target-triple> -- SpmcSync/Cons-4_Cap-1_Items-100000
  ```

* **All benchmarks in suite:**

  ```bash
  RUSTFLAGS="-Z sanitizer=thread" cargo +nightly bench --bench spmc_sync --target <target-triple>
  ```

---

### 5. Notes & Troubleshooting

* **Cross‑Compilation:** Ensure your target toolchain is installed if testing on non‑native architectures:

  ```bash
  rustup target add x86_64-apple-darwin --toolchain nightly
  ```

* **Verbose TSan Output:** Use `-Z sanitizer=thread -- -Zsanitize=thread` if you need extra debug information.

* **CI Integration:** Export `RUSTFLAGS` and specify `+nightly` in your pipeline steps to keep your local and CI environments consistent.

* **Resetting RUSTFLAGS:** Unset when done:

  ```bash
  unset RUSTFLAGS
  ```

---

For more details, visit the [Rust Sanitizers Book](https://doc.rust-lang.org/unstable-book/compiler-flags/sanitizer.html).
