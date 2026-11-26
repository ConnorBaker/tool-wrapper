//! Byte-oriented quoting primitives for argument files and text formats.

/// What an escape means in the caller's grammar.
pub enum Escape {
    Keep,
    Drop,
    Reject,
}

/// Split on ASCII whitespace outside the supplied quote characters.
///
/// Backslash consumes the next byte. The caller decides how that escape
/// behaves given the active quote. Unfinished quotes, trailing backslashes,
/// and rejected escapes return `None`.
pub fn split_words(
    input: &[u8],
    quotes: &[u8],
    mut escape: impl FnMut(Option<u8>, u8) -> Escape,
) -> Option<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut word = None::<Vec<u8>>;
    let mut quote = None;
    let mut bytes = input.iter().copied();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\\' => {
                let escaped = bytes.next()?;
                let word = word.get_or_insert_with(Vec::new);
                match escape(quote, escaped) {
                    Escape::Keep => word.push(escaped),
                    Escape::Drop => {}
                    Escape::Reject => return None,
                }
            }
            byte if quote == Some(byte) => quote = None,
            byte if quote.is_none() && quotes.contains(&byte) => {
                quote = Some(byte);
                word.get_or_insert_with(Vec::new);
            }
            byte if quote.is_none() && (byte.is_ascii_whitespace() || byte == 0x0b) => {
                words.extend(word.take());
            }
            byte => word.get_or_insert_with(Vec::new).push(byte),
        }
    }
    if quote.is_some() {
        return None;
    }
    words.extend(word);
    Some(words)
}

/// Append a double-quoted word, escaping backslashes and double quotes.
pub fn push_double_quoted(text: &mut Vec<u8>, word: &[u8]) {
    text.push(b'"');
    for &byte in word {
        if matches!(byte, b'\\' | b'"') {
            text.push(b'\\');
        }
        text.push(byte);
    }
    text.push(b'"');
}

/// Encode one escaped, double-quoted word per line.
pub fn double_quoted_lines(words: &[Vec<u8>]) -> Vec<u8> {
    let mut text = Vec::new();
    for word in words {
        push_double_quoted(&mut text, word);
        text.push(b'\n');
    }
    text
}
