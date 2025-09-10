### 1. First-Class Support for Markers (Advanced Filtering)

#### The "Why": Beyond Log Levels

Log levels (`INFO`, `WARN`, `ERROR`) are excellent for filtering by severity, but they can't capture the *semantic meaning* or *business context* of a log event. A log message might be `INFO` level but represent a critical security event that needs to be written to a dedicated audit log.

**Markers** (a concept from SLF4J/Logback) solve this by acting as "tags" you can attach to a log event. This allows you to create filtering and routing rules based on the *purpose* of the log, independent of its severity.

**Use Cases:**
*   **Security Auditing:** Tag all login attempts, permission changes, and resource access events with a `SECURITY_AUDIT` marker and route them to a secure, tamper-proof log file.
*   **Performance Monitoring:** Tag the start and end of expensive operations with a `PERFORMANCE` marker to calculate durations and analyze bottlenecks.
*   **Billing Events:** Tag all events related to billable actions (e.g., API calls, data processed) with a `BILLING` marker to be consumed by a separate financial system.

#### The User Experience

**In Rust Code:** The most idiomatic way to add a marker is to establish a convention using a special field name within the `tracing` macros.

```rust
use tracing::info;

// A standard informational log
info!(user_id = 123, "User profile updated successfully.");

// Same log, but now tagged with a marker for auditing purposes
info!(
    marker = "SECURITY_AUDIT", // The special marker field
    user_id = 123,
    action = "update_profile",
    "User profile updated successfully."
);

// Another example for performance
info!(
    marker = "PERFORMANCE",
    task_name = "image_resize",
    duration_ms = 152,
    "Complex task finished."
);
```

**In `fibre_logging.yaml`:** You would configure an appender with a new `filter` section that specifically looks for these markers.

```yaml
appenders:
  # A general-purpose appender for application logs
  main_log_file:
    kind: file
    path: "logs/app.log"
    # ...

  # A dedicated, secure appender ONLY for security audit events
  security_audit_log:
    kind: file
    path: "logs/security_audit.log"
    encoder:
      kind: json_lines # Audit logs should be structured
    
    # NEW: Filter configuration
    filter:
      kind: marker
      marker: "SECURITY_AUDIT"
      # on_match: The action to take if the event has the marker. ACCEPT means process it.
      on_match: "ACCEPT"
      # on_mismatch: The action if the event does NOT have the marker. DENY means drop it immediately.
      on_mismatch: "DENY"

loggers:
  root:
    level: info
    # All logs go to the main file, but only audited ones go to the security file.
    appenders: [main_log_file, security_audit_log]
```

#### Implementation Sketch

1.  **`LogEvent` Enhancement:** The `LogEventFieldVisitor` would be updated to recognize the special `"marker"` field. Instead of putting it in the generic `fields` map, it would populate a new, dedicated field on the `LogEvent` struct for fast access:
    ```rust
    // in src/model.rs
    pub struct LogEvent {
        // ... existing fields ...
        pub marker: Option<String>,
    }
    ```
2.  **Filter Configuration:** New structs would be added to `config/raw.rs` and `config/processed.rs` to represent the `filter` block in the YAML.
3.  **Filtering Logic:**
    *   Introduce a `Filter` trait and a `FilterReply` enum.
        ```rust
        enum FilterReply { Accept, Deny, Neutral }
        trait Filter {
            fn decide(&self, event: &LogEvent) -> FilterReply;
        }
        ```
    *   The `EventProcessor` would be the central point for filtering. Before checking the logger's level, it would first run the event through any configured appender filters.
    *   If any filter returns `Deny`, the event is dropped for that appender.
    *   If any filter returns `Accept`, the event is processed immediately, bypassing level checks.
    *   If all filters return `Neutral` (the default), processing continues to the normal level-based filtering.

---

### 2. Expand the Appender Ecosystem

The current appenders are foundational. The next step is to add appenders that integrate with common infrastructure for centralized logging and alerting.

#### TCP Appender

*   **Why:** For sending logs to a centralized log aggregator like **Logstash**, **Fluentd**, or **Vector**. This is standard practice in distributed systems. It's more reliable and performant than tailing log files.
*   **User Experience (`yaml`):**
    ```yaml
    appenders:
      logstash:
        kind: tcp
        # The address of the log aggregator
        address: "127.0.0.1:5044"
        encoder:
          # Aggregators almost always consume structured formats
          kind: json_lines
    ```
*   **Implementation Sketch:**
    *   In `init.rs`, a new `AppenderKindInternal::Tcp` variant would be handled.
    *   The background writer thread for this appender would not open a file but would instead establish a `std::net::TcpStream` to the configured address.
    *   The core loop would call `stream.write_all(&bytes)` on received log messages.
    *   **Crucially**, it must include robust reconnection logic. If the stream write fails, it should close the connection and attempt to reconnect with an exponential backoff strategy to avoid spamming a down service.

#### Webhook Appender (for Slack, Discord, etc.)

*   **Why:** To get immediate notifications for critical errors in production. This is the modern, vastly superior alternative to Logback's old `SMTPAppender` (emailing on error).
*   **User Experience (`yaml`):**
    ```yaml
    appenders:
      slack_alerts:
        kind: webhook
        # The webhook URL provided by Slack/Discord/etc.
        url: "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX"
        # Only send alerts for ERROR level events
        level_threshold: "ERROR"
        # A simple template for the message payload
        template: "🚨 *{level}* in `{target}`:\n```{message}```"
    ```
*   **Implementation Sketch:**
    *   The background writer thread would use a blocking HTTP client like `ureq` or `reqwest::blocking`.
    *   It would receive a `LogEvent` (not formatted bytes, as it needs the raw data for the template).
    *   It would apply the template to the `LogEvent` data to create the message body.
    *   It would construct the JSON payload required by the webhook provider (e.g., `{"text": "..."}` for Slack).
    *   It would POST this payload to the configured URL.
    *   It should include internal rate-limiting to prevent a flood of errors from spamming the channel (e.g., "send at most 1 alert every 10 seconds, and then a summary message").

---

### 5. Formalize Spans as the "MDC" Equivalent

`tracing`'s spans are the modern, async-safe successor to Logback's Mapped Diagnostic Context (MDC). The goal is to automatically enrich every log event with the contextual data from the span(s) it occurred in.

*   **Why:** To avoid manually repeating contextual information (`request_id`, `user_id`, `trace_id`) on every single log statement. This makes logs vastly more useful for debugging and analysis.

#### The User Experience

**In Rust Code:** The user does *nothing different*. They just use `tracing` spans as intended. This is the beauty of the feature—it requires zero changes to application instrumentation code.

```rust
use tracing::{info, info_span, Instrument};

#[instrument(name="process_request", fields(request_id = "xyz-789", user.id = 42))]
fn handle_request() {
    // This log event will automatically be enriched with
    // `request_id` and `user.id` from the span.
    info!("Processing payment for user."); 
}
```

**In `fibre_logging.yaml`:** The user enables this feature in the encoder configuration.

```yaml
appenders:
  json_file:
    kind: file
    path: "logs/app_context.json"
    encoder:
      kind: json_lines
      # NEW: This flag tells the encoder to walk the span stack
      # and include all contextual fields.
      include_span_fields: true

  console_pattern:
    kind: console
    encoder:
      kind: pattern
      # NEW: A Logback-style specifier to pull a specific field from the context.
      pattern: "[%d] %p {request_id=%X{request_id}} %t - %m%n"
```
*When `handle_request` is called, the console output would be:*
`[2023-10-27T10:30:00.123Z] INFO {request_id=xyz-789} my_app::handlers - Processing payment for user.`

#### Implementation Sketch

This is the most complex of the three but provides the most value for `tracing` users.

1.  **Span Data Collection:** The core logic would live in `subscriber/dispatch.rs`.
    *   The `DispatchLayer::build_log_event` function already receives the `Context<'_, S>`.
    *   It would use `ctx.lookup_current()` to get the current span.
    *   It would then need to **walk the span hierarchy** from the current span up to the root (`span.parent()`).
    *   For each span in the hierarchy, it must extract all of its fields. This requires using a custom `tracing::field::Visit` implementation to record the values from each span's `ValueSet`.
2.  **`LogEvent` Enhancement:** The `LogEvent` struct would gain a new field to hold this merged context.
    ```rust
    // in src/model.rs
    pub struct LogEvent {
        // ...
        // A map of all fields collected from the active span context.
        pub span_context: HashMap<String, LogValue>,
    }
    ```
    Care must be taken when merging fields: an inner span's field should override a field with the same name from an outer span.
3.  **Encoder Integration:**
    *   **`JsonLinesFormatter`:** If `include_span_fields` is `true`, it would merge the `event.span_context` map into the final JSON object.
    *   **`PatternFormatter`:** It would need to be updated to parse the new `%X{key}` or `%mdc{key}` conversion specifier. When it encounters this, it would look up `key` in the `event.span_context` map and write the value.