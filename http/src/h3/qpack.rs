//! QPACK field sections with the static table only.
//!
//! The client advertises a dynamic table capacity of 0, so a field section from the server may only use the static table and literals (RFC 9204 section 3.2.3). Requests are encoded the same way.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::tables::{HUFFMAN, STATIC};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DecodeError(pub &'static str);

struct Index {
  exact: HashMap<(&'static str, &'static str), usize>,
  name: HashMap<&'static str, usize>,
}

fn index() -> &'static Index {
  static INDEX: OnceLock<Index> = OnceLock::new();
  INDEX.get_or_init(|| {
    let mut exact = HashMap::new();
    let mut name = HashMap::new();
    for (i, &(n, v)) in STATIC.iter().enumerate() {
      exact.entry((n, v)).or_insert(i);
      name.entry(n).or_insert(i);
    }
    Index { exact, name }
  })
}

/// Starts a field section: Required Insert Count 0 and Delta Base 0.
pub(crate) fn begin(out: &mut Vec<u8>) {
  out.extend_from_slice(&[0x00, 0x00]);
}

/// Appends one field line. `name` is lowercase.
pub(crate) fn field(out: &mut Vec<u8>, name: &str, value: &[u8]) {
  let idx = index();
  if let Ok(v) = std::str::from_utf8(value) {
    if let Some(&i) = idx.exact.get(&(name, v)) {
      put_int(out, 0xc0, 6, i as u64);
      return;
    }
  }
  match idx.name.get(name) {
    Some(&i) => {
      put_int(out, 0x50, 4, i as u64);
      put_int(out, 0x00, 7, value.len() as u64);
      out.extend_from_slice(value);
    }
    None => {
      put_int(out, 0x20, 3, name.len() as u64);
      out.extend_from_slice(name.as_bytes());
      put_int(out, 0x00, 7, value.len() as u64);
      out.extend_from_slice(value);
    }
  }
}

/// Decodes a field section into name and value pairs.
pub(crate) fn decode(mut b: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, DecodeError> {
  let short = DecodeError("short field section");
  let dynamic = DecodeError("dynamic table reference with no dynamic table");
  if get_int(&mut b, 8).ok_or(short)? != 0 {
    return Err(dynamic);
  }
  get_int(&mut b, 7).ok_or(DecodeError("short field section"))?;
  let mut out = Vec::with_capacity(8);
  while let Some(&first) = b.first() {
    if first & 0x80 != 0 {
      if first & 0x40 == 0 {
        return Err(dynamic);
      }
      let i = get_int(&mut b, 6).ok_or(DecodeError("short index"))? as usize;
      let (n, v) = STATIC.get(i).ok_or(DecodeError("static index out of range"))?;
      out.push((n.as_bytes().to_vec(), v.as_bytes().to_vec()));
    } else if first & 0x40 != 0 {
      if first & 0x10 == 0 {
        return Err(dynamic);
      }
      let i = get_int(&mut b, 4).ok_or(DecodeError("short index"))? as usize;
      let (n, _) = STATIC.get(i).ok_or(DecodeError("static index out of range"))?;
      let v = get_string(&mut b, 7)?;
      out.push((n.as_bytes().to_vec(), v));
    } else if first & 0x20 != 0 {
      let n = get_string(&mut b, 3)?;
      let v = get_string(&mut b, 7)?;
      out.push((n, v));
    } else {
      return Err(dynamic);
    }
  }
  Ok(out)
}

fn get_string(b: &mut &[u8], prefix: u32) -> Result<Vec<u8>, DecodeError> {
  let huffman = b.first().ok_or(DecodeError("short string"))? & (1 << prefix) != 0;
  let len = get_int(b, prefix).ok_or(DecodeError("short string"))? as usize;
  if b.len() < len {
    return Err(DecodeError("short string"));
  }
  let raw = &b[..len];
  *b = &b[len..];
  if huffman { huffman_decode(raw) } else { Ok(raw.to_vec()) }
}

/// An RFC 7541 section 5.1 integer with a `prefix`-bit prefix. The bits above the prefix in the first byte are flags and are skipped.
fn get_int(b: &mut &[u8], prefix: u32) -> Option<u64> {
  let mask = ((1u16 << prefix) - 1) as u8;
  let first = *b.first()? & mask;
  *b = &b[1..];
  if first < mask {
    return Some(first as u64);
  }
  let mut v = mask as u64;
  let mut shift = 0;
  loop {
    let byte = *b.first()?;
    *b = &b[1..];
    v = v.checked_add(((byte & 0x7f) as u64) << shift)?;
    if byte & 0x80 == 0 {
      return Some(v);
    }
    shift += 7;
    if shift > 56 {
      return None;
    }
  }
}

fn put_int(out: &mut Vec<u8>, flags: u8, prefix: u32, v: u64) {
  let mask = ((1u16 << prefix) - 1) as u64;
  if v < mask {
    out.push(flags | v as u8);
    return;
  }
  out.push(flags | mask as u8);
  let mut rest = v - mask;
  while rest >= 0x80 {
    out.push((rest as u8 & 0x7f) | 0x80);
    rest >>= 7;
  }
  out.push(rest as u8);
}

/// The Huffman code as a binary tree: children are node indices and a leaf is `-(byte + 1)`.
fn huffman_tree() -> &'static [[i32; 2]] {
  static TREE: OnceLock<Vec<[i32; 2]>> = OnceLock::new();
  TREE.get_or_init(|| {
    let mut tree = vec![[0i32; 2]];
    for (sym, &(code, len)) in HUFFMAN.iter().enumerate() {
      let mut node = 0usize;
      for i in (0..len).rev() {
        let bit = ((code >> i) & 1) as usize;
        if i == 0 {
          tree[node][bit] = -(sym as i32 + 1);
        } else {
          if tree[node][bit] <= 0 {
            tree.push([0, 0]);
            tree[node][bit] = (tree.len() - 1) as i32;
          }
          node = tree[node][bit] as usize;
        }
      }
    }
    tree
  })
}

fn huffman_decode(src: &[u8]) -> Result<Vec<u8>, DecodeError> {
  let tree = huffman_tree();
  let mut out = Vec::with_capacity(src.len() * 8 / 5);
  let mut node = 0usize;
  let mut depth = 0u32;
  let mut ones = true;
  for &byte in src {
    for i in (0..8).rev() {
      let bit = ((byte >> i) & 1) as usize;
      ones &= bit == 1;
      depth += 1;
      match tree[node][bit] {
        next if next < 0 => {
          out.push((-next - 1) as u8);
          node = 0;
          depth = 0;
          ones = true;
        }
        0 => return Err(DecodeError("invalid Huffman code")),
        next => node = next as usize,
      }
    }
  }
  // RFC 7541 section 5.2: padding is under 8 bits, all ones.
  if depth > 7 || !ones {
    return Err(DecodeError("invalid Huffman padding"));
  }
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn huffman_decodes_rfc_7541_c_4_1() {
    let enc = [0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff];
    assert_eq!(huffman_decode(&enc).unwrap(), b"www.example.com");
  }

  #[test]
  fn huffman_rejects_padding_of_a_byte_or_more() {
    assert!(huffman_decode(&[0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff, 0xff]).is_err());
  }

  #[test]
  fn encoded_fields_decode_to_themselves() {
    let fields: [(&str, &[u8]); 6] =
      [(":method", b"GET"), (":scheme", b"https"), (":authority", b"example.test:8443"), (":path", b"/a/long/path?q=1"), ("accept-encoding", b"gzip, zstd"), ("x-custom-header-with-a-long-name", &[b'v'; 300])];
    let mut block = Vec::new();
    begin(&mut block);
    for (n, v) in fields {
      field(&mut block, n, v);
    }
    let decoded = decode(&block).unwrap();
    let expected: Vec<(Vec<u8>, Vec<u8>)> = fields.iter().map(|(n, v)| (n.as_bytes().to_vec(), v.to_vec())).collect();
    assert_eq!(decoded, expected);
  }

  #[test]
  fn an_exact_static_match_is_one_byte() {
    let mut block = Vec::new();
    field(&mut block, ":status", b"200");
    assert_eq!(block, [0xc0 | 25]);
  }

  #[test]
  fn a_dynamic_table_reference_is_refused() {
    assert_eq!(decode(&[0x01, 0x00]), Err(DecodeError("dynamic table reference with no dynamic table")));
    assert_eq!(decode(&[0x00, 0x00, 0x80]), Err(DecodeError("dynamic table reference with no dynamic table")));
    assert_eq!(decode(&[0x00, 0x00, 0x10]), Err(DecodeError("dynamic table reference with no dynamic table")));
  }

  #[test]
  fn a_huffman_literal_from_a_peer_decodes() {
    let block = [0x00, 0x00, 0x5f, 92 - 15, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff];
    let fields = decode(&block).unwrap();
    assert_eq!(fields, vec![(b"server".to_vec(), b"www.example.com".to_vec())]);
  }
}
