use fibre_stats::{FieldKind, FieldRole, field, fields, find_field};

field! {
  /// The request a line belongs to.
  #[kind(ulid)]
  #[role(correlation)]
  #[lives_secs(300)]
  pub static REQUEST_ID: Field = "request_id";

  /// The account a request acted for.
  #[role(entity)]
  #[kind(id64)]
  static ACCOUNT: Field = "account_id";

  static NOTE: Field = "note";
}

#[test]
fn every_declaration_is_registered_with_its_kind_role_and_description() {
  assert_eq!(REQUEST_ID.value("01J9ZQ4XGH8N0V2C7K5M3R6T1Y"), "01J9ZQ4XGH8N0V2C7K5M3R6T1Y");
  assert_eq!(ACCOUNT.value(42u64), 42);
  assert_eq!(NOTE.value("x"), "x");

  let request = find_field("request_id").unwrap();
  assert_eq!((request.kind, request.role), (Some(FieldKind::Ulid), FieldRole::Correlation));
  assert_eq!(request.description, &[" The request a line belongs to."]);
  assert_eq!(request.lives, Some(std::time::Duration::from_secs(300)));
  assert_eq!(find_field("account_id").unwrap().lives, None);
  let account = find_field("account_id").unwrap();
  assert_eq!((account.kind, account.role), (Some(FieldKind::Id64), FieldRole::Entity), "attributes in either order");
  let note = find_field("note").unwrap();
  assert_eq!((note.kind, note.role), (None, FieldRole::Plain));
  assert!(find_field("nothing").is_none());
  assert_eq!(fields().filter(|field| ["request_id", "account_id", "note"].contains(&field.name)).count(), 3);
}
