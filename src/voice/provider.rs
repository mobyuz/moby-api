use moby_protocol::{Language, Script};

use super::{Locale, Transcript, wave::SAMPLE_RATE};

pub(super) struct ProviderError;
pub(super) type ProviderResult<T> = Result<(T, i64), ProviderError>;

pub(super) trait SpeechProvider {
    // Cost estimates are local and perform no provider work.
    fn estimate_stt_cost(&self, seconds: u32) -> i64;
    fn estimate_tts_cost(&self, characters: u32) -> i64;

    async fn transcribe(&self, pcm: &[u8], locale: Locale) -> ProviderResult<Transcript>;
    async fn synthesize(&self, text: &str, locale: Locale) -> ProviderResult<Vec<u8>>;
}

pub(super) struct MockSpeech;

impl SpeechProvider for MockSpeech {
    fn estimate_stt_cost(&self, _: u32) -> i64 {
        0
    }

    fn estimate_tts_cost(&self, _: u32) -> i64 {
        0
    }

    async fn transcribe(&self, _: &[u8], locale: Locale) -> ProviderResult<Transcript> {
        let text = match (locale.language, locale.script) {
            (Language::Uz, Script::Latn) => "Salom, Mobi.",
            (Language::Uz, Script::Cyrl) => "Салом, Моби.",
            (Language::Ru, _) => "Привет, Моби.",
            (Language::En, _) => "Hello, Moby.",
        };
        Ok((
            Transcript {
                text: text.into(),
                language: locale.language,
                script: locale.script,
                confidence: 1.0,
            },
            0,
        ))
    }

    async fn synthesize(&self, text: &str, locale: Locale) -> ProviderResult<Vec<u8>> {
        let frequency = match (locale.language, locale.script) {
            (Language::Uz, Script::Latn) => 440,
            (Language::Uz, Script::Cyrl) => 494,
            (Language::Ru, _) => 554,
            (Language::En, _) => 622,
        };
        let frames = (text.chars().count() * 160).clamp(1600, 8000);
        let mut pcm = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let sample: i16 = if (frame * frequency * 2 / SAMPLE_RATE).is_multiple_of(2) {
                1200
            } else {
                -1200
            };
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        Ok((pcm, 0))
    }
}
