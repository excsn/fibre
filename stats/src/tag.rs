//! Tags name the series a value belongs to.

use std::borrow::Cow;

/// A metric or tag name. Borrowed when it is a literal, owned when built at runtime.
pub type Name = Cow<'static, str>;

/// One `name = value` pair on a series.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag {
  pub name: Name,
  pub value: Name,
}

impl Tag {
  pub fn new(name: impl Into<Name>, value: impl Into<Name>) -> Self {
    Tag {
      name: name.into(),
      value: value.into(),
    }
  }
}

/// The tags on one observation or series.
pub type Tags = Vec<Tag>;

/// Sorts tags by name. When a name repeats, the last value wins.
pub fn normalize(tags: &mut Tags) {
  tags.sort_by(|a, b| a.name.cmp(&b.name));
  tags.dedup_by(|later, earlier| {
    if later.name == earlier.name {
      std::mem::swap(later, earlier);
      true
    } else {
      false
    }
  });
}

/// Builds a `Tags` from `name => value` pairs.
///
/// ```
/// use fibre_stats::tags;
/// let route = String::from("/cart");
/// let t = tags!("status" => "ok", "route" => route);
/// assert_eq!(t.len(), 2);
/// ```
#[macro_export]
macro_rules! tags {
  () => {
    $crate::Tags::new()
  };
  ($($name:expr => $value:expr),+ $(,)?) => {
    vec![$($crate::Tag::new($name, $value)),+]
  };
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalize_sorts_and_keeps_the_last_duplicate() {
    let mut tags = tags!("b" => "1", "a" => "x", "b" => "2");
    normalize(&mut tags);
    assert_eq!(tags, vec![Tag::new("a", "x"), Tag::new("b", "2")]);
  }

  #[test]
  fn empty_tags_macro() {
    let tags: Tags = tags!();
    assert!(tags.is_empty());
  }
}
