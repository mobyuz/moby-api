# Bounded voice requests

The desktop sends a complete RIFF/WAVE clip with one channel of signed 16-bit
PCM at 16 kHz. This matches the planned capture format. The STT request allows
up to 30 seconds of PCM and 4 KiB of WAV overhead. The provider receives only
the PCM frames. The TTS request accepts up to 500 Unicode scalar values and
returns the same WAV format. Locale hints support Uzbek in Latin and Cyrillic,
Russian in Cyrillic, and English in Latin. [Microsoft PCM format](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/ns-mmeapi-waveformatex)

The reservation transaction checks the current `cloud_voice` consent under a
row lock before admitting provider work. A withdrawal that acquires the row
first blocks the new request. Work already admitted can complete. The
transaction also reserves whole STT seconds or TTS characters and the upper
monetary charge. LLM and voice share the daily and monthly service budget.
Unknown charges retain their reservations after timeout or disconnection.
Only IDs, amounts, meter kinds, and dates are written to the usage ledger.

Migration 0006 preserves existing B4 reservations as `llm` and retains their
budget buckets. Stop existing B4 API processes before running the B5 schema
upgrade so every running process uses the new table names.

The local mock returns a locale fixture for STT and a deterministic signal for
TTS. Development Compose enables it for contract checks. A host process enables
it through `MOBY_VOICE_ENABLED`. The provider interface allows an adapter to
supply bounded monetary estimates and report the final charge.
