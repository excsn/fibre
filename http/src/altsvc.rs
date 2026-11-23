//! The `Alt-Svc` response header (RFC 7838): another place an origin can be reached, read here for HTTP/3 only.

use std::time::Duration;

/// RFC 7838 section 3.1: an alternative with no `ma` is fresh for 24 hours.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AltSvc {
  /// `clear`: the origin has no alternatives.
  Clear,
  /// The first `h3` alternative. An empty `host` is the origin's own host.
  H3 { host: String, port: u16, max_age: Duration },
  /// Nothing for HTTP/3.
  Other,
}

pub(crate) fn parse(value: &str) -> AltSvc {
  let value = value.trim();
  if value.eq_ignore_ascii_case("clear") {
    return AltSvc::Clear;
  }
  for entry in split_unquoted(value, ',') {
    let mut parts = split_unquoted(entry, ';').into_iter();
    let Some((protocol, authority)) = parts.next().and_then(|p| p.split_once('=')) else { continue };
    if protocol.trim() != "h3" {
      continue;
    }
    let authority = authority.trim().trim_matches('"');
    let Some((host, port)) = authority.rsplit_once(':') else { continue };
    let Ok(port) = port.parse::<u16>() else { continue };
    if port == 0 {
      continue;
    }
    let mut max_age = DEFAULT_MAX_AGE;
    for param in parts {
      if let Some((name, v)) = param.split_once('=') {
        if name.trim().eq_ignore_ascii_case("ma") {
          if let Ok(secs) = v.trim().trim_matches('"').parse::<u64>() {
            max_age = Duration::from_secs(secs);
          }
        }
      }
    }
    let host = host.trim_start_matches('[').trim_end_matches(']').to_owned();
    return AltSvc::H3 { host, port, max_age };
  }
  AltSvc::Other
}

/// Splits on `sep` outside double quotes.
fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
  let mut parts = Vec::new();
  let mut quoted = false;
  let mut start = 0;
  for (i, c) in s.char_indices() {
    match c {
      '"' => quoted = !quoted,
      c if c == sep && !quoted => {
        parts.push(&s[start..i]);
        start = i + c.len_utf8();
      }
      _ => {}
    }
  }
  parts.push(&s[start..]);
  parts
}

#[cfg(test)]
mod tests {
  use super::*;

  fn h3(host: &str, port: u16, secs: u64) -> AltSvc {
    AltSvc::H3 { host: host.into(), port, max_age: Duration::from_secs(secs) }
  }

  #[test]
  fn the_same_host_on_another_port() {
    assert_eq!(parse(r#"h3=":443"; ma=3600"#), h3("", 443, 3600));
  }

  #[test]
  fn no_max_age_means_a_day() {
    assert_eq!(parse(r#"h3=":8443""#), h3("", 8443, 86400));
  }

  #[test]
  fn the_first_h3_entry_wins_over_drafts_and_other_protocols() {
    assert_eq!(parse(r#"h3-29=":443"; ma=10, h2="alt.test:443", h3="alt.test:9443"; ma=60; persist=1, h3=":1""#), h3("alt.test", 9443, 60));
  }

  #[test]
  fn an_ipv6_host_loses_its_brackets() {
    assert_eq!(parse(r#"h3="[::1]:443""#), h3("::1", 443, 86400));
  }

  #[test]
  fn clear_is_recognised_in_any_case() {
    assert_eq!(parse(" Clear "), AltSvc::Clear);
  }

  #[test]
  fn a_quoted_separator_does_not_split() {
    assert_eq!(parse(r#"h2="a,b:1", h3=":2""#), h3("", 2, 86400));
  }

  #[test]
  fn values_with_no_usable_h3_entry_are_other() {
    for v in ["", r#"h2=":443""#, r#"h3=":0""#, r#"h3=":x""#, r#"h3="noport""#, "garbage"] {
      assert_eq!(parse(v), AltSvc::Other, "{v}");
    }
  }
}
