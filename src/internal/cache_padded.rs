// src/internal/cache_padded.rs

//! Utility for cache line padding.

use core::fmt;
use core::ops::{Deref, DerefMut};

// Define specific aligned inner types for common cache line sizes.
// Add more if needed for other architectures or specific tuning.

#[repr(C)]
#[repr(align(64))]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
struct AlignedInner64<T> {
  value: T,
}

#[repr(C)]
#[repr(align(128))]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
struct AlignedInner128<T> {
  value: T,
}

// Add AlignedInner32 if you ever expect such small cache lines or need that specific alignment.
// #[repr(C)]
// #[repr(align(32))]
// #[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
// struct AlignedInner32<T> {
//     value: T,
// }

// Conditionally compiled module to select the alignment value and type alias.
#[cfg(target_arch = "x86_64")]
mod arch_details {
  pub const CACHE_LINE_SIZE_USIZE: usize = 64;
  // The type alias that uses the specific AlignedInner struct.
  pub type ArchAligned<T> = crate::internal::cache_padded::AlignedInner64<T>;
}

#[cfg(target_arch = "aarch64")]
mod arch_details {
  // AArch64 can have 64 or 128. Let's default to 64 for broader compatibility
  // but one might choose 128 for specific high-performance ARM targets.
  // This could be a feature flag within the aarch64 cfg.
  pub const CACHE_LINE_SIZE_USIZE: usize = 64;
  pub type ArchAligned<T> = crate::internal::cache_padded::AlignedInner64<T>;
  // Example if you wanted to use 128 for aarch64:
  // pub const CACHE_LINE_SIZE_USIZE: usize = 128;
  // pub type ArchAligned<T> = crate::internal::cache_padded::AlignedInner128<T>;
}

// Default for other architectures.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod arch_details {
  pub const CACHE_LINE_SIZE_USIZE: usize = 64; // A reasonable default.
  pub type ArchAligned<T> = crate::internal::cache_padded::AlignedInner64<T>;
}

/// A type `T` padded to the length of a cache line.
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
pub(crate) struct CachePadded<T> {
  // This inner field holds the appropriately aligned data.
  inner: arch_details::ArchAligned<T>,
}

impl<T> CachePadded<T> {
  /// Creates a new cache-padded value.
  #[inline]
  pub(crate) const fn new(value: T) -> Self {
    CachePadded {
      // Construct the specific ArchAligned type.
      inner: arch_details::ArchAligned { value },
    }
  }

  /// Returns the actual cache line size used for padding for the current architecture.
  #[inline]
  pub(crate) const fn alignment_value() -> usize {
    arch_details::CACHE_LINE_SIZE_USIZE
  }
}

impl<T> Deref for CachePadded<T> {
  type Target = T;
  #[inline]
  fn deref(&self) -> &T {
    &self.inner.value
  }
}

impl<T> DerefMut for CachePadded<T> {
  #[inline]
  fn deref_mut(&mut self) -> &mut T {
    &mut self.inner.value
  }
}

impl<T: fmt::Debug> fmt::Debug for CachePadded<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CachePadded")
      // self.value uses Deref to access self.inner.value
      .field("value", &self.inner.value)
      .field("alignment", &Self::alignment_value())
      .finish()
  }
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

#[cfg(test)]
mod tests {
  use super::*;
  use core::mem;

  #[test]
  fn alignment_check() {
    #[derive(Debug)] // Added for assert messages
    struct MyData(u64);
    let padded_data = CachePadded::new(MyData(0));
    let ptr = &padded_data as *const _ as usize;

    let expected_alignment = CachePadded::<MyData>::alignment_value();
    assert_eq!(
      mem::align_of_val(&padded_data),
      expected_alignment,
      "CachePadded struct alignment mismatch. Expected {}, got {}",
      expected_alignment,
      mem::align_of_val(&padded_data)
    );
    assert_eq!(
      ptr % expected_alignment,
      0,
      "CachePadded instance address 0x{:x} is not aligned to {}",
      ptr,
      expected_alignment
    );

    let size = mem::size_of_val(&padded_data);
    assert!(size >= mem::size_of::<MyData>());
    // The size of CachePadded<T> will be the size of T rounded up to the nearest multiple
    // of the alignment, but at least the alignment size if T is smaller.
    // More precisely, it will be `max(align_of_val(&padded_data), size_of::<T>())` then
    // potentially rounded up further if T itself is large and causes the padded struct to
    // cross multiple cache lines but still needs to end on an alignment boundary (less common).
    // A simpler check is that it's at least the alignment if T is small.
    if mem::size_of::<MyData>() <= expected_alignment {
      assert_eq!(
        size, expected_alignment,
        "Size of CachePadded should be the alignment size when T is small enough. Expected {}, got {}",
        expected_alignment, size
      );
    } else {
      // If T is larger than a cache line, the size will be T rounded up to a multiple of the alignment.
      assert_eq!(
        size % expected_alignment,
        0,
        "Size of CachePadded should be a multiple of alignment when T is large"
      );
      assert!(size >= mem::size_of::<MyData>());
    }
  }

  #[test]
  fn const_constructor() {
    static _PADDED_CONST: CachePadded<u32> = CachePadded::new(42);
    assert_eq!(*_PADDED_CONST, 42); // Check the value too
  }

  #[test]
  fn debug_output() {
    let p = CachePadded::new(10i32);
    let s = format!("{:?}", p);
    assert!(s.contains("CachePadded"));
    assert!(s.contains("value: 10"));
    assert!(s.contains(&format!("alignment: {}", CachePadded::<i32>::alignment_value())));
  }

  #[test]
  fn deref_mut_works() {
    let mut p = CachePadded::new(String::from("hello"));
    p.push_str(" world");
    assert_eq!(*p, "hello world");
  }
}
