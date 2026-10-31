fibre_stats::metric! {
  /// Nothing refers to this handle.
  pub static UNREFERENCED: Counter = "fixture.dep.unreferenced";
}

pub mod recording {
  fibre_stats::metric! {
    /// Recorded by the function the program calls.
    pub static RECORDED: Counter = "fixture.dep.recorded";
  }

  #[inline(never)]
  pub fn work(n: u64) -> u64 {
    RECORDED.add(n, fibre_stats::tags!());
    std::hint::black_box(n) + 1
  }
}

pub mod handle {
  fibre_stats::metric! {
    /// The program refers to this handle and never records it.
    pub static HANDLE: Counter = "fixture.dep.handle";
  }
}
