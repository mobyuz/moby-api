//! Server-Sent Events framing for OpenRouter streams. Comment lines such as
//! `: OPENROUTER PROCESSING` are keep-alives and never become events.

// Bounds a single line and a single event so a hostile stream cannot grow memory.
const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Malformed {
    TooLarge,
    NotUtf8,
}

#[derive(Default)]
pub(super) struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    has_data: bool,
}

impl Decoder {
    /// Appends network bytes and returns each completed event's `data` payload.
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, Malformed> {
        let mut events = Vec::new();
        for &byte in bytes {
            if byte != b'\n' {
                self.line.push(byte);
                if self.line.len() > MAX_BYTES {
                    return Err(Malformed::TooLarge);
                }
                continue;
            }
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            let line = std::mem::take(&mut self.line);
            if line.is_empty() {
                if std::mem::take(&mut self.has_data) {
                    let data = std::mem::take(&mut self.data);
                    events.push(String::from_utf8(data).map_err(|_| Malformed::NotUtf8)?);
                }
            } else if line[0] != b':' {
                let (field, value) = match line.iter().position(|&byte| byte == b':') {
                    Some(colon) => (&line[..colon], &line[colon + 1..]),
                    None => (&line[..], &[][..]),
                };
                if field == b"data" {
                    if self.has_data {
                        self.data.push(b'\n');
                    }
                    self.has_data = true;
                    self.data
                        .extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
                    if self.data.len() > MAX_BYTES {
                        return Err(Malformed::TooLarge);
                    }
                }
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_arbitrary_chunks_and_skips_comments() {
        let wire = ": OPENROUTER PROCESSING\n\ndata: {\"a\":1}\n\n: keep-alive\r\n\r\ndata: x\r\ndata: y\r\n\r\ndata: [DONE]\n\n";
        let whole = Decoder::default().push(wire.as_bytes()).unwrap();
        assert_eq!(whole, ["{\"a\":1}", "x\ny", "[DONE]"]);
        for size in 1..wire.len() {
            let mut decoder = Decoder::default();
            let mut events = Vec::new();
            for chunk in wire.as_bytes().chunks(size) {
                events.extend(decoder.push(chunk).unwrap());
            }
            assert_eq!(events, whole, "chunk size {size}");
        }
    }

    #[test]
    fn keeps_multibyte_text_split_between_chunks() {
        let wire = "data: Салом\n\n".as_bytes();
        let mut decoder = Decoder::default();
        let mut events = decoder.push(&wire[..8]).unwrap();
        events.extend(decoder.push(&wire[8..]).unwrap());
        assert_eq!(events, ["Салом"]);
    }

    #[test]
    fn bounds_lines_and_rejects_invalid_utf8() {
        assert_eq!(
            Decoder::default().push(&vec![b'a'; MAX_BYTES + 1]),
            Err(Malformed::TooLarge)
        );
        assert_eq!(
            Decoder::default().push(b"data: \xff\n\n"),
            Err(Malformed::NotUtf8)
        );
    }
}
