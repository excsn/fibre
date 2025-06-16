use fibre_ioc::{global, resolve};
use std::sync::Arc;

// --- Abstraction and Implementations ---
trait MessageSender: Send + Sync {
  fn send(&self, to: &str, message: &str) -> String;
}

struct EmailSender;
impl MessageSender for EmailSender {
  fn send(&self, to: &str, message: &str) -> String {
    format!("Sending email to {}: '{}'", to, message)
  }
}

struct SmsSender;
impl MessageSender for SmsSender {
  fn send(&self, to: &str, message: &str) -> String {
    format!("Sending SMS to {}: '{}'", to, message)
  }
}

fn main() {
  // --- Registration ---
  // Register both implementations with unique names.
  global().add_singleton_trait_with_name::<dyn MessageSender>("email", || Arc::new(EmailSender));
  global().add_singleton_trait_with_name::<dyn MessageSender>("sms", || Arc::new(SmsSender));

  // --- Resolution ---
  // Now we can choose which implementation we want at the point of resolution.
  let email_notifier = resolve!(trait MessageSender, "email");
  let sms_notifier = resolve!(trait MessageSender, "sms");

  let result1 = email_notifier.send("test@example.com", "Hello from Fibre!");
  let result2 = sms_notifier.send("+123456789", "Hello from Fibre!");

  println!("{}", result1);
  println!("{}", result2);

  assert!(result1.contains("email"));
  assert!(result2.contains("SMS"));
}
