pub(super) const SAMPLE_RATE: usize = 16_000;
pub(super) const BYTE_RATE: usize = SAMPLE_RATE * 2;
const MAX_SECONDS: usize = 30;
const MAX_PCM_BYTES: usize = BYTE_RATE * MAX_SECONDS;
const MAX_NON_AUDIO_BYTES: usize = 4096;
pub(super) const MAX_WAV_BYTES: usize = MAX_PCM_BYTES + MAX_NON_AUDIO_BYTES;

// PCM, one channel, 16 kHz, 32 kB/s, two bytes per frame, 16 bits.
const PCM_FORMAT: [u8; 16] = [1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaveError {
    Invalid,
    TooLarge,
}

pub(super) fn pcm(wave: &[u8]) -> Result<&[u8], WaveError> {
    if wave.len() > MAX_WAV_BYTES {
        return Err(WaveError::TooLarge);
    }
    let header = wave.get(..12).ok_or(WaveError::Invalid)?;
    if &header[..4] != b"RIFF"
        || read_size(&header[4..8]) != wave.len() - 8
        || &header[8..] != b"WAVE"
    {
        return Err(WaveError::Invalid);
    }
    let mut remaining = &wave[12..];
    let mut format_seen = false;
    let mut audio = None;
    while !remaining.is_empty() {
        let chunk = remaining.get(..8).ok_or(WaveError::Invalid)?;
        let size = read_size(&chunk[4..8]);
        let bytes = remaining.get(8..).ok_or(WaveError::Invalid)?;
        let contents = bytes.get(..size).ok_or(WaveError::Invalid)?;
        remaining = bytes.get(size..).ok_or(WaveError::Invalid)?;
        if !size.is_multiple_of(2) {
            remaining = remaining.get(1..).ok_or(WaveError::Invalid)?;
        }
        match &chunk[..4] {
            b"fmt " if !format_seen => {
                if contents.get(..PCM_FORMAT.len()) != Some(PCM_FORMAT.as_slice())
                    || !matches!(contents.get(PCM_FORMAT.len()..), Some([] | [0, 0]))
                {
                    return Err(WaveError::Invalid);
                }
                format_seen = true;
            }
            b"data" if format_seen && audio.is_none() => audio = Some(contents),
            b"fmt " | b"data" => return Err(WaveError::Invalid),
            _ => {}
        }
    }
    let audio = audio.ok_or(WaveError::Invalid)?;
    if wave.len() - audio.len() > MAX_NON_AUDIO_BYTES {
        return Err(WaveError::TooLarge);
    }
    validate_pcm(audio)?;
    Ok(audio)
}

pub(super) fn encode(audio: Vec<u8>) -> Result<Vec<u8>, WaveError> {
    validate_pcm(&audio)?;
    let size = u32::try_from(audio.len()).expect("bounded PCM size");
    let mut wave = Vec::with_capacity(44 + audio.len());
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + size).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16_u32.to_le_bytes());
    wave.extend_from_slice(&PCM_FORMAT);
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&size.to_le_bytes());
    wave.extend_from_slice(&audio);
    Ok(wave)
}

fn validate_pcm(audio: &[u8]) -> Result<(), WaveError> {
    if audio.len() > MAX_PCM_BYTES {
        Err(WaveError::TooLarge)
    } else if audio.is_empty() || !audio.len().is_multiple_of(2) {
        Err(WaveError::Invalid)
    } else {
        Ok(())
    }
}

fn read_size(bytes: &[u8]) -> usize {
    u32::from_le_bytes(bytes.try_into().expect("four RIFF length bytes")) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_checks_lengths_format_and_padding() {
        let audio = vec![0, 1, 2, 3];
        let valid = encode(audio.clone()).unwrap();
        assert_eq!(pcm(&valid).unwrap(), audio);

        let mut padded = valid.clone();
        padded.splice(12..12, *b"JUNK\x01\0\0\0x\0");
        let size = u32::try_from(padded.len() - 8).unwrap();
        padded[4..8].copy_from_slice(&size.to_le_bytes());
        assert_eq!(pcm(&padded).unwrap(), audio);

        let mut stereo = valid.clone();
        stereo[22] = 2;
        assert_eq!(pcm(&stereo), Err(WaveError::Invalid));
        let mut duplicate = valid.clone();
        duplicate.extend_from_slice(b"data\x02\0\0\0\0\0");
        let size = u32::try_from(duplicate.len() - 8).unwrap();
        duplicate[4..8].copy_from_slice(&size.to_le_bytes());
        assert_eq!(pcm(&duplicate), Err(WaveError::Invalid));
        assert_eq!(pcm(&valid[..valid.len() - 1]), Err(WaveError::Invalid));
        assert_eq!(encode(vec![0; MAX_PCM_BYTES + 2]), Err(WaveError::TooLarge));
        assert_eq!(encode(vec![0]), Err(WaveError::Invalid));
    }
}
