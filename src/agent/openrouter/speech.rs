//! Turns streamed model text into `speech_delta` pieces and the final
//! expression. The prompt asks the model to end a spoken reply with
//! `<expression>NAME</expression>`. That tag never reaches the desktop as speech.

use moby_protocol::Expression;

const OPEN: &str = "<expression>";
// The agent route accepts 32 deltas. The last slot belongs to the final flush.
const MAX_DELTAS: usize = 30;
// Release a long clause without punctuation so speech does not wait for the end.
const MAX_PENDING_BYTES: usize = 160;

pub(super) struct Speech {
    held: String,
    marker: Option<String>,
    said: String,
    deltas: usize,
}

pub(super) struct Spoken {
    pub(super) deltas: Vec<String>,
    pub(super) text: String,
    pub(super) expression: Expression,
}

impl Speech {
    pub(super) fn new() -> Self {
        Self {
            held: String::new(),
            marker: None,
            said: String::new(),
            deltas: 0,
        }
    }

    pub(super) fn push(&mut self, text: &str) -> Vec<String> {
        if let Some(marker) = &mut self.marker {
            marker.push_str(text);
            return Vec::new();
        }
        let text = if self.said.is_empty() && self.held.is_empty() {
            text.trim_start()
        } else {
            text
        };
        self.held.push_str(text);
        if let Some(start) = self.held.find(OPEN) {
            self.marker = Some(self.held.split_off(start + OPEN.len()));
            self.held.truncate(start);
        }
        // Keep a possible partial tag until the next chunk shows what it is.
        let keep = if self.marker.is_some() {
            0
        } else {
            (1..OPEN.len())
                .rev()
                .find(|&len| self.held.ends_with(&OPEN[..len]))
                .unwrap_or(0)
        };
        let region = &self.held[..self.held.len() - keep];
        let cut = boundary(region);
        if cut > 0 && self.deltas + 1 < MAX_DELTAS && !region[..cut].trim().is_empty() {
            return vec![self.release(cut)];
        }
        Vec::new()
    }

    pub(super) fn finish(mut self) -> Spoken {
        let mut deltas = Vec::new();
        let rest = self.held.trim_end().len();
        if !self.held[..rest].trim().is_empty() {
            deltas.push(self.release(rest));
        }
        let expression = self
            .marker
            .as_deref()
            .map(|marker| marker.split('<').next().unwrap_or_default())
            .map(|name| name.trim().to_ascii_lowercase())
            .and_then(|name| match name.as_str() {
                "idle" => Some(Expression::Idle),
                "happy" => Some(Expression::Happy),
                "confused" => Some(Expression::Confused),
                // Other faces are system states, not model choices.
                _ => None,
            })
            .unwrap_or_default();
        Spoken {
            deltas,
            text: self.said,
            expression,
        }
    }

    fn release(&mut self, cut: usize) -> String {
        let piece: String = self.held.drain(..cut).collect();
        self.said.push_str(&piece);
        self.deltas += 1;
        piece
    }
}

// Returns where the last complete sentence or line ends, or 0.
fn boundary(region: &str) -> usize {
    let mut cut = 0;
    let mut chars = region.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let space_follows = chars.peek().is_some_and(|(_, next)| next.is_whitespace());
        if ch == '\n' {
            cut = index;
        } else if matches!(ch, '.' | '!' | '?' | '…' | '。') && space_follows {
            cut = index + ch.len_utf8();
        }
    }
    if cut == 0 && region.len() >= MAX_PENDING_BYTES {
        cut = region
            .rmatch_indices(char::is_whitespace)
            .next()
            .map_or(0, |(index, _)| index);
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> (Vec<String>, Spoken) {
        let mut speech = Speech::new();
        let mut deltas: Vec<String> = chunks.iter().flat_map(|chunk| speech.push(chunk)).collect();
        let spoken = speech.finish();
        deltas.extend(spoken.deltas.iter().cloned());
        (deltas, spoken)
    }

    #[test]
    fn releases_sentences_and_strips_the_expression_tag() {
        let (deltas, spoken) = run(&[
            "  Salom! Men ",
            "Mobiman. Yordam ",
            "bera olaman.\n<expr",
            "ession>hap",
            "py</expression>\n",
        ]);
        assert_eq!(deltas.concat(), "Salom! Men Mobiman. Yordam bera olaman.");
        assert_eq!(spoken.text, deltas.concat());
        assert_eq!(spoken.expression, Expression::Happy);
        assert!(deltas.iter().all(|delta| !delta.contains('<')));
        assert!(deltas.len() >= 3, "text should stream before the end");
    }

    #[test]
    fn unknown_or_missing_expression_falls_back_to_idle() {
        for tail in ["<expression>error</expression>", "<expression>", ""] {
            let (_, spoken) = run(&["Hello there.", tail]);
            assert_eq!(spoken.text, "Hello there.");
            assert_eq!(spoken.expression, Expression::Idle, "{tail:?}");
        }
        let (_, spoken) = run(&["Sorry.<expression>CONFUSED</expression>"]);
        assert_eq!(spoken.expression, Expression::Confused);
    }

    #[test]
    fn a_partial_tag_is_speech_when_it_never_completes() {
        let (deltas, spoken) = run(&["2 < 3 and <expr"]);
        assert_eq!(deltas.concat(), "2 < 3 and <expr");
        assert_eq!(spoken.text, "2 < 3 and <expr");
    }

    #[test]
    fn caps_the_delta_count_and_keeps_all_text() {
        let sentence = "Hi. ";
        let chunks: Vec<&str> = std::iter::repeat_n(sentence, 100).collect();
        let (deltas, spoken) = run(&chunks);
        assert!(deltas.len() <= MAX_DELTAS, "{}", deltas.len());
        assert_eq!(spoken.text, deltas.concat());
        assert_eq!(spoken.text, chunks.concat().trim_end());
    }

    #[test]
    fn splits_long_unpunctuated_text_and_handles_cyrillic() {
        let long = "слово ".repeat(60);
        let mut chunks: Vec<&str> = long.split_inclusive(' ').collect();
        chunks.push("<expression>happy</expression>");
        let (deltas, spoken) = run(&chunks);
        assert!(deltas.len() > 1);
        assert_eq!(spoken.text, long.trim_end());
        assert!(deltas.iter().all(|delta| !delta.trim().is_empty()));
    }
}
