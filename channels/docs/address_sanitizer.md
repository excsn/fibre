## Sanitizing Rust Code with ASan & Miri

A concise guide to verifying memory safety using Address Sanitizer (ASan) and the Miri interpreter on macOS using the nightly toolchain.

---

### 1. Prerequisites

*   **Rustup**: Ensure `rustup` is installed.
*   **Nightly Toolchain**: Required for sanitizer flags and Miri.
*   **Xcode Command‑Line Tools**: Provides the clang runtime required for ASan.

---

### 2. Install Components (One-time setup)

1.  **Install Nightly:**
    ```bash
    rustup toolchain install nightly
    ```
2.  **Install Miri Component:**
    ```bash
    rustup +nightly component add miri
    ```

---

### 3. Part A: Address Sanitizer (ASan)

ASan detects **Use-After-Free**, **Double-Free**, **Buffer Overflows**, and **Memory Leaks**. It is faster than Miri and runs your actual compiled binary.

**1. The Command:**

```bash
RUSTFLAGS="-Z sanitizer=address" \
cargo +nightly test \
    -Z build-std \
    --target aarch64-apple-darwin \
    mpsc::block_queue
```

*(Replace `aarch64-apple-darwin` with `x86_64-apple-darwin` for Intel Macs).*

**Why `-Z build-std`?**
This recompiles the Rust standard library with ASan enabled. This is **critical** for finding bugs involving standard types like `Arc`, `Box`, or `Vec`, as it allows the sanitizer to see inside the allocator logic.

**Pro Tip:**
If you see "LeakSanitizer" errors regarding `malloc` internals on macOS, you can suppress leak detection to focus on memory corruption:
```bash
ASAN_OPTIONS=detect_leaks=0 ...
```

---

### 4. Part B: Miri Interpreter

Miri detects **Undefined Behavior (UB)** that ASan might miss, such as **Invalid Aliasing** (Stacked Borrows), **Uninitialized Memory usage**, and **Data Races**. It interprets the code line-by-line (very slow but thorough).

**1. The Command:**

```bash
cargo +nightly miri test mpsc::block_queue
```

**2. Running with strict flags (Optional):**
To catch even more subtle bugs (like incorrect tree borrows):

```bash
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test
```

**Note on Performance:**
Miri is an interpreter. Tests that take milliseconds in ASan might take seconds or minutes in Miri. You may need to reduce iteration counts in stress tests using `#[cfg(miri)]`.

---

### 5. Comparison & Workflow

| Feature | **ASan (AddressSanitizer)** | **Miri** |
| :--- | :--- | :--- |
| **Detects** | Buffer overflows, Use-after-free | UB, Invalid Aliasing, Uninit Memory |
| **Speed** | ~2x slowdown | ~100x+ slowdown |
| **Scope** | Compiled Binary (Real CPU) | Abstract Rust Machine (Virtual) |
| **FFI** | Works with C code | Limited (often fails on C calls) |

**Recommended Workflow:**
1.  **Run Miri First:** Use it on unit tests to ensure your pointer logic and `unsafe` blocks are theoretically sound.
2.  **Run ASan Second:** Use it on stress tests and integration tests to catch runtime memory corruption under heavy load.

---

### 6. Troubleshooting

*   **"Signal 6 / SIGABRT"**: Usually means the sanitizer caught an error. Scroll up in the console output to see the stack trace provided by the sanitizer.
*   **Target Mismatch**: If `cargo` complains about missing std for the target, ensure you have the target installed:
    ```bash
    rustup target add aarch64-apple-darwin --toolchain nightly
    ```
*   **Miri Timeout**: If a test hangs in Miri, it might be too intense. Reduce loop counts:
    ```rust
    #[test]
    fn stress_test() {
        // Use smaller count for Miri
        const ITERS: usize = if cfg!(miri) { 100 } else { 50_000 };
        // ...
    }
    ```