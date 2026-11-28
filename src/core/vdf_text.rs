//! Span-keeping VDF text lexer.

/// One lexical token of VDF text.
#[derive(Clone, Copy)]
pub(crate) enum Tok<'a> {
    /// Quoted string, with its start offset.
    Str(usize, &'a str),
    /// Opening brace offset.
    Open(usize),
    /// Closing brace offset.
    Close(usize),
}

/// Lex quoted strings and braces.
pub(crate) fn vdf_tokens(raw: &str) -> Vec<Tok<'_>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    // Skip escaped characters.
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                out.push(Tok::Str(start, raw.get(start + 1..end).unwrap_or("")));
            }
            b'{' => out.push(Tok::Open(i)),
            b'}' => out.push(Tok::Close(i)),
            _ => {}
        }
        i += 1;
    }
    out
}
