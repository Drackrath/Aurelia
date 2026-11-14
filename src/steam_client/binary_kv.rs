//! Binary KeyValues, flattened.

/// Binary KeyValues type bytes.
mod kv {
    pub const SECTION: u8 = 0;
    pub const STRING: u8 = 1;
    pub const INT32: u8 = 2;
    pub const FLOAT32: u8 = 3;
    pub const POINTER: u8 = 4;
    pub const COLOR: u8 = 6;
    pub const UINT64: u8 = 7;
    pub const END: u8 = 8;
    pub const INT64: u8 = 10;
    pub const END_ALT: u8 = 11;
}

/// One section of string pairs.
pub fn encode(section: &str, pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = vec![kv::SECTION];
    out.extend_from_slice(section.as_bytes());
    out.push(0);
    for (key, value) in pairs {
        out.push(kv::STRING);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    out.push(kv::END);
    out.push(kv::END);
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn cstr(&mut self) -> Option<String> {
        let end = self.0.iter().position(|&b| b == 0)?;
        let text = String::from_utf8_lossy(&self.0[..end]).into_owned();
        self.0 = &self.0[end + 1..];
        Some(text)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
}

/// Leaves of one section, flattened.
fn read_section(r: &mut Reader, out: &mut Vec<(String, String)>, depth: usize) -> Option<()> {
    if depth > 16 {
        return None;
    }
    loop {
        let kind = r.byte()?;
        if kind == kv::END || kind == kv::END_ALT {
            return Some(());
        }
        let name = r.cstr()?;
        let value = match kind {
            kv::SECTION => {
                read_section(r, out, depth + 1)?;
                continue;
            }
            kv::STRING => r.cstr()?,
            kv::INT32 | kv::POINTER | kv::COLOR => {
                i32::from_le_bytes(r.take(4)?.try_into().ok()?).to_string()
            }
            kv::FLOAT32 => f32::from_le_bytes(r.take(4)?.try_into().ok()?).to_string(),
            kv::UINT64 => u64::from_le_bytes(r.take(8)?.try_into().ok()?).to_string(),
            kv::INT64 => i64::from_le_bytes(r.take(8)?.try_into().ok()?).to_string(),
            _ => return None,
        };
        out.push((name, value));
    }
}

/// Key/value pairs; None when malformed.
pub fn decode(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    decode_prefix(bytes).map(|(pairs, _)| pairs)
}

/// First document's leaves and its length.
pub fn decode_prefix(bytes: &[u8]) -> Option<(Vec<(String, String)>, usize)> {
    let mut out = Vec::new();
    let mut r = Reader(bytes);
    if r.byte()? != kv::SECTION {
        return None;
    }
    r.cstr()?;
    read_section(&mut r, &mut out, 0)?;
    // Optional end of top level.
    if r.0.first() == Some(&kv::END) {
        r.byte();
    }
    Some((out, bytes.len() - r.0.len()))
}
