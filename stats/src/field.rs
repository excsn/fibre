//! Field declarations: what a log or trace field holds and what it is for.
//!
//! A declaration is written once with [`field!`](crate::field!) beside the code that writes the field. A store
//! reads [`fields`] to learn that `request_id` is a ULID that links records, so it can keep it as 128 bits and
//! keep a filter for it, with nothing measured from the data.

/// How a field's text is kept. A value that does not parse as the declared form is kept as it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldKind {
  /// 26 Crockford base32 characters, kept as 128 bits.
  Ulid,
  /// 36 characters with hyphens, kept as 128 bits.
  Uuid,
  /// A decimal number below 2^64, kept as one 64-bit number.
  Id64,
}

impl FieldKind {
  pub fn as_str(&self) -> &'static str {
    match self {
      FieldKind::Ulid => "ulid",
      FieldKind::Uuid => "uuid",
      FieldKind::Id64 => "id64",
    }
  }
}

/// What a field is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldRole {
  Plain,
  /// Short-lived and close to unique, such as a trace or request id. Looked up by exact value.
  Correlation,
  /// Long-lived and repeated, such as an account.
  Entity,
}

impl FieldRole {
  pub fn as_str(&self) -> &'static str {
    match self {
      FieldRole::Plain => "plain",
      FieldRole::Correlation => "correlation",
      FieldRole::Entity => "entity",
    }
  }
}

/// A field declaration and the handle a log call names its value through.
#[derive(Debug)]
pub struct Field {
  pub name: &'static str,
  pub kind: Option<FieldKind>,
  pub role: FieldRole,
  pub description: &'static [&'static str],
  /// For a correlation id: the most time between the first and the last record that carry one value.
  pub lives: Option<std::time::Duration>,
}

impl Field {
  pub const fn new(name: &'static str, kind: Option<FieldKind>, role: FieldRole, description: &'static [&'static str]) -> Self {
    Field { name, kind, role, description, lives: None }
  }

  pub const fn lives(mut self, lives: Option<std::time::Duration>) -> Self {
    self.lives = lives;
    self
  }

  /// Passes `value` through unchanged. A log call that writes the field through this keeps the declaration in the
  /// program: the linker drops a declaration nothing refers to, and a field name alone refers to nothing.
  ///
  /// ```
  /// fibre_stats::field! {
  ///   #[kind(ulid)]
  ///   pub static REQUEST_ID: Field = "request_id";
  /// }
  /// let id = "01J9ZQ4XGH8N0V2C7K5M3R6T1Y";
  /// assert_eq!(REQUEST_ID.value(id), id);
  /// ```
  #[inline(never)]
  pub fn value<T>(&'static self, value: T) -> T {
    std::hint::black_box(self);
    value
  }
}

#[doc(hidden)]
pub struct RegisteredField(pub &'static Field);

inventory::collect!(RegisteredField);

/// Every `field!` declaration the linker kept, in no defined order. A declaration a log call writes through with
/// [`Field::value`] is always kept. Two crates may declare the same name; both appear.
pub fn fields() -> impl Iterator<Item = &'static Field> {
  inventory::iter::<RegisteredField>.into_iter().map(|registered| registered.0)
}

/// The first declaration from [`fields`] with this name.
pub fn find_field(name: &str) -> Option<&'static Field> {
  fields().find(|field| field.name == name)
}
