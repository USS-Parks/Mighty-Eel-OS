//! Speech-to-Text Handoff Interface for the MAI.
//!
//! Manages the STT pipeline:
//! - Audio input validation and format negotiation
//! - Whisper model management through the model registry
//! - Streaming audio frame aggregation from WebSocket
//! - Transcription result routing (partial and final)
//! - Transcription-to-chat pipeline (audio -> text -> inference)
//!
//! # Architecture
//!
//! The SttManager does NOT run Whisper. It validates audio input,
//! manages the transcription request lifecycle, and packages results
//! for the chat completion pipeline. The actual Whisper inference
//! goes through the standard scheduler -> adapter path.
//!
//! Whisper is treated as a Sentinel-tier model: it runs in Sentinel
//! power mode alongside the small language model, consuming ~4GB VRAM.
//!
//! # Air-Gap Safety
//!
//! All audio processing is local. No cloud transcription services.

use std::collections::HashMap;
use std::time::Instant;

use tracing::info;
use uuid::Uuid;

use mai_core::types::{ModelId, ProfileId, RequestId};

use crate::types::{
    AgentError, AudioEncoding, AudioFormat, PartialTranscription, SttConfig, Transcription,
    WordTimestamp,
};

// ============================================================================
// Audio Buffer
// ============================================================================

/// Aggregates streaming audio frames into complete utterances.
#[derive(Debug)]
struct AudioBuffer {
    /// Raw audio bytes accumulated
    data: Vec<u8>,
    /// Audio format metadata
    format: AudioFormat,
    /// Total duration of audio accumulated (milliseconds)
    duration_ms: u64,
    /// Maximum allowed duration (milliseconds)
    max_duration_ms: u64,
    /// Silence detection: consecutive silence frames
    silence_frames: u32,
    /// Silence threshold in frames
    silence_threshold: u32,
    /// Absolute cap on accumulated bytes. Bounds memory regardless of
    /// reported frame duration — sub-millisecond / sub-sample frames floor to zero
    /// duration, so the duration guard alone cannot stop an unbounded byte flood.
    max_bytes: usize,
}

impl AudioBuffer {
    fn new(format: AudioFormat, max_duration_secs: u32, silence_threshold_ms: u32) -> Self {
        // Calculate silence threshold in frames
        let bytes_per_sample = u64::from(format.bit_depth) / 8;
        let bytes_per_second = u64::from(format.sample_rate)
            .saturating_mul(u64::from(format.channels))
            .saturating_mul(bytes_per_sample);
        let frame_duration_ms = 20u32; // 20ms frames typical for WebSocket audio
        let silence_frames = silence_threshold_ms / frame_duration_ms;
        // Absolute byte cap tied to the duration budget: the bytes `max_duration`
        // seconds of this format would occupy.
        let max_bytes = bytes_per_second
            .saturating_mul(u64::from(max_duration_secs))
            .try_into()
            .unwrap_or(usize::MAX);

        Self {
            data: Vec::new(),
            format,
            duration_ms: 0,
            max_duration_ms: u64::from(max_duration_secs) * 1000,
            silence_frames: 0,
            silence_threshold: silence_frames,
            max_bytes,
        }
    }

    /// Append an audio frame. Returns true if the buffer is ready for
    /// transcription (silence detected or max duration reached).
    fn append_frame(&mut self, frame: &[u8]) -> Result<bool, AgentError> {
        // Reject malformed frames before any duration arithmetic: an
        // empty frame carries no audio, and a frame that is not a whole number of
        // samples both mis-aligns the stream and (sub-sample) reports zero duration
        // while still consuming bytes. Guarding `frame_align != 0` also avoids a
        // divide-by-zero on a degenerate (bit_depth / channels == 0) format.
        let bytes_per_sample = usize::from(self.format.bit_depth) / 8;
        let frame_align = bytes_per_sample * usize::from(self.format.channels);
        if frame.is_empty() {
            return Err(AgentError::MalformedAudioFrame("empty frame".to_owned()));
        }
        if frame_align == 0 || !frame.len().is_multiple_of(frame_align) {
            return Err(AgentError::MalformedAudioFrame(format!(
                "{} bytes is not a whole number of {frame_align}-byte samples",
                frame.len()
            )));
        }

        // Absolute byte cap: bounds memory even against a flood of zero-duration
        // frames the duration guard below cannot catch (sub-ms frames floor to 0).
        let new_len = self.data.len().saturating_add(frame.len());
        if new_len > self.max_bytes {
            return Err(AgentError::AudioBytesExceeded {
                bytes: new_len,
                max_bytes: self.max_bytes,
            });
        }

        // Check duration limit
        let frame_duration_ms = self.frame_duration_ms(frame.len());
        if self.duration_ms + frame_duration_ms > self.max_duration_ms {
            return Err(AgentError::AudioDurationExceeded {
                duration_ms: self.duration_ms + frame_duration_ms,
                max_ms: self.max_duration_ms,
            });
        }

        // Simple silence detection: check if frame is below threshold
        let is_silence = is_silent_frame(frame, &self.format);
        if is_silence {
            self.silence_frames += 1;
        } else {
            self.silence_frames = 0;
        }

        self.data.extend_from_slice(frame);
        self.duration_ms += frame_duration_ms;

        // Ready if silence threshold reached and we have some audio
        Ok(self.silence_frames >= self.silence_threshold && self.duration_ms > 500)
    }

    /// Calculate frame duration in milliseconds from byte length.
    fn frame_duration_ms(&self, frame_bytes: usize) -> u64 {
        let bytes_per_sample = u64::from(self.format.bit_depth) / 8;
        let channels = u64::from(self.format.channels);
        let sample_rate = u64::from(self.format.sample_rate);

        let samples = frame_bytes as u64 / (bytes_per_sample * channels);
        if sample_rate == 0 {
            return 0;
        }
        (samples * 1000) / sample_rate
    }

    /// Get total accumulated bytes.
    fn byte_count(&self) -> usize {
        self.data.len()
    }

    /// Take the buffer contents, leaving it empty.
    fn take(&mut self) -> Vec<u8> {
        self.duration_ms = 0;
        self.silence_frames = 0;
        std::mem::take(&mut self.data)
    }
}

/// Simple silence detection: check if RMS amplitude is below threshold.
fn is_silent_frame(frame: &[u8], format: &AudioFormat) -> bool {
    if frame.is_empty() || format.encoding != AudioEncoding::Pcm || format.bit_depth != 16 {
        return false;
    }

    // Parse 16-bit PCM samples (little-endian)
    let mut sum_sq = 0.0f64;
    let mut sample_count = 0u32;

    let mut i = 0;
    while i + 1 < frame.len() {
        let sample = i16::from_le_bytes([frame[i], frame[i + 1]]);
        sum_sq += f64::from(sample) * f64::from(sample);
        sample_count += 1;
        i += 2;
    }

    if sample_count == 0 {
        return true;
    }

    let rms = (sum_sq / f64::from(sample_count)).sqrt();
    // Threshold: ~1% of max amplitude (32768 * 0.01 = ~328)
    rms < 328.0
}

// ============================================================================
// STT Manager
// ============================================================================

/// Transcription request state.
#[derive(Debug)]
struct TranscriptionRequest {
    /// Unique request ID
    id: RequestId,
    /// Profile making the request
    profile_id: ProfileId,
    /// Audio buffer
    buffer: AudioBuffer,
    /// Model to use for transcription
    model: ModelId,
    /// Language hint (None = auto-detect)
    language_hint: Option<String>,
    /// Whether streaming partials are enabled
    streaming: bool,
    /// Partial transcriptions received so far
    partials: Vec<PartialTranscription>,
    /// When the request was created
    created_at: Instant,
}

/// Manages speech-to-text transcription requests.
///
/// Thread safety: NOT internally synchronized. Wrap in Arc<RwLock<_>>.
pub struct SttManager {
    /// Configuration
    config: SttConfig,
    /// Active transcription requests indexed by request ID
    active_requests: HashMap<RequestId, TranscriptionRequest>,
    /// Metrics
    metrics: SttMetrics,
}

/// STT performance metrics.
#[derive(Debug, Clone, Default)]
pub struct SttMetrics {
    /// Total transcription requests
    pub total_requests: u64,
    /// Completed transcriptions
    pub completed: u64,
    /// Failed transcriptions
    pub failed: u64,
    /// Total audio duration processed (milliseconds)
    pub total_audio_ms: u64,
    /// Total bytes of audio processed
    pub total_audio_bytes: u64,
}

impl SttManager {
    /// Create a new STT manager.
    pub fn new(config: SttConfig) -> Self {
        Self {
            config,
            active_requests: HashMap::new(),
            metrics: SttMetrics::default(),
        }
    }

    /// Start a new transcription request.
    ///
    /// Returns the request ID. Audio frames are fed via `feed_audio_frame()`.
    pub fn start_transcription(
        &mut self,
        profile_id: ProfileId,
        format: Option<AudioFormat>,
        language_hint: Option<String>,
        streaming: bool,
    ) -> Result<RequestId, AgentError> {
        let request_id = Uuid::new_v4();
        let audio_format = format.unwrap_or(AudioFormat {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            bit_depth: self.config.bit_depth,
            encoding: AudioEncoding::Pcm,
        });

        // Validate audio format
        self.validate_format(&audio_format)?;

        let buffer = AudioBuffer::new(
            audio_format,
            self.config.max_duration_secs,
            self.config.silence_threshold_ms,
        );

        let request = TranscriptionRequest {
            id: request_id,
            profile_id,
            buffer,
            model: self.config.default_model.clone(),
            language_hint: language_hint.or_else(|| self.config.language_hint.clone()),
            streaming,
            partials: Vec::new(),
            created_at: Instant::now(),
        };

        self.active_requests.insert(request_id, request);
        self.metrics.total_requests += 1;

        info!(%request_id, %profile_id, "Started transcription request");
        Ok(request_id)
    }

    /// Feed an audio frame to an active transcription request.
    ///
    /// Returns true if the buffer is ready for transcription
    /// (silence detected or max duration reached).
    pub fn feed_audio_frame(
        &mut self,
        request_id: &RequestId,
        frame: &[u8],
    ) -> Result<bool, AgentError> {
        let request = self.active_requests.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active transcription: {request_id}"))
        })?;

        request.buffer.append_frame(frame)
    }

    /// Get the accumulated audio data for a request (for sending to Whisper).
    ///
    /// Returns the audio bytes, format metadata, and language hint.
    /// The buffer is consumed (taken); feed more frames for the next utterance.
    pub fn take_audio(
        &mut self,
        request_id: &RequestId,
    ) -> Result<(Vec<u8>, AudioFormat, Option<String>), AgentError> {
        let request = self.active_requests.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active transcription: {request_id}"))
        })?;

        let audio = request.buffer.take();
        let format = request.buffer.format.clone();
        let language = request.language_hint.clone();

        self.metrics.total_audio_bytes += audio.len() as u64;

        Ok((audio, format, language))
    }

    /// Record a partial transcription result (for streaming STT).
    pub fn record_partial(
        &mut self,
        request_id: &RequestId,
        partial: PartialTranscription,
    ) -> Result<(), AgentError> {
        let request = self.active_requests.get_mut(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active transcription: {request_id}"))
        })?;
        request.partials.push(partial);
        Ok(())
    }

    /// Complete a transcription request with the final result.
    ///
    /// Removes the request from active tracking and returns the
    /// final Transcription for the chat pipeline.
    pub fn complete_transcription(
        &mut self,
        request_id: &RequestId,
        text: String,
        language: String,
        confidence: f32,
        words: Vec<WordTimestamp>,
    ) -> Result<Transcription, AgentError> {
        let request = self.active_requests.remove(request_id).ok_or_else(|| {
            AgentError::Internal(format!("No active transcription: {request_id}"))
        })?;
        debug_assert_eq!(request.id, *request_id);

        let duration_ms = {
            #[allow(clippy::cast_possible_truncation)]
            let val = request.created_at.elapsed().as_millis() as u64;
            val
        };

        let transcription = Transcription {
            text,
            language,
            confidence,
            duration_ms,
            words,
            model: request.model.clone(),
        };

        self.metrics.completed += 1;
        self.metrics.total_audio_ms += request.buffer.duration_ms;

        info!(
            %request_id,
            profile_id = %request.profile_id,
            duration_ms,
            model = %request.model,
            streaming = request.streaming,
            "Transcription completed"
        );

        Ok(transcription)
    }

    /// Cancel an active transcription request.
    pub fn cancel_transcription(&mut self, request_id: &RequestId) -> bool {
        let removed = self.active_requests.remove(request_id).is_some();
        if removed {
            self.metrics.failed += 1;
            info!(%request_id, "Transcription cancelled");
        }
        removed
    }

    /// Check if a request is actively being transcribed.
    pub fn is_active(&self, request_id: &RequestId) -> bool {
        self.active_requests.contains_key(request_id)
    }

    /// Get the audio buffer size for a request.
    pub fn buffer_size(&self, request_id: &RequestId) -> Option<usize> {
        self.active_requests
            .get(request_id)
            .map(|r| r.buffer.byte_count())
    }

    /// Get the number of partial transcriptions for a request.
    pub fn partial_count(&self, request_id: &RequestId) -> Option<usize> {
        self.active_requests
            .get(request_id)
            .map(|r| r.partials.len())
    }

    /// Count active transcription requests.
    pub fn active_count(&self) -> usize {
        self.active_requests.len()
    }

    /// Get metrics.
    pub fn metrics(&self) -> &SttMetrics {
        &self.metrics
    }

    /// Get the default STT model name.
    pub fn default_model(&self) -> &str {
        &self.config.default_model
    }

    /// Validate an audio format against supported configurations.
    #[allow(clippy::unused_self)]
    fn validate_format(&self, format: &AudioFormat) -> Result<(), AgentError> {
        // Validate sample rate
        match format.sample_rate {
            8000 | 16000 | 22050 | 44100 | 48000 => {}
            other => {
                return Err(AgentError::UnsupportedAudioFormat(format!(
                    "Unsupported sample rate: {other}Hz"
                )));
            }
        }

        // Validate channels
        if format.channels == 0 || format.channels > 2 {
            return Err(AgentError::UnsupportedAudioFormat(format!(
                "Unsupported channel count: {}",
                format.channels
            )));
        }

        // Validate bit depth
        match format.bit_depth {
            8 | 16 | 24 | 32 => {}
            other => {
                return Err(AgentError::UnsupportedAudioFormat(format!(
                    "Unsupported bit depth: {other}"
                )));
            }
        }

        // Validate encoding
        match format.encoding {
            AudioEncoding::Pcm | AudioEncoding::Wav | AudioEncoding::Flac | AudioEncoding::Opus => {
            }
        }

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SttConfig {
        SttConfig {
            default_model: "whisper-large-v3".into(),
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            max_duration_secs: 60,
            streaming_enabled: true,
            silence_threshold_ms: 500,
            language_hint: None,
        }
    }

    fn pcm_frame(amplitude: i16, samples: usize) -> Vec<u8> {
        let mut frame = Vec::with_capacity(samples * 2);
        for _ in 0..samples {
            frame.extend_from_slice(&amplitude.to_le_bytes());
        }
        frame
    }

    #[test]
    fn test_start_transcription() {
        let mut mgr = SttManager::new(test_config());
        let profile = Uuid::new_v4();
        let id = mgr.start_transcription(profile, None, None, true).unwrap();
        assert!(mgr.is_active(&id));
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_feed_audio_frames() {
        let mut mgr = SttManager::new(test_config());
        let id = mgr
            .start_transcription(Uuid::new_v4(), None, None, false)
            .unwrap();

        // Feed a non-silent frame
        let frame = pcm_frame(5000, 320); // 320 samples = 20ms at 16kHz
        let ready = mgr.feed_audio_frame(&id, &frame).unwrap();
        assert!(!ready); // Not enough silence yet

        assert!(mgr.buffer_size(&id).unwrap() > 0);
    }

    #[test]
    fn zero_duration_frame_flood_is_byte_bounded() {
        // A single 16-bit sample at 16 kHz floors to 0 ms
        // (1000 / 16000 == 0), so the duration guard never trips; the byte cap
        // must bound memory instead. 1-second budget => 32 000-byte cap.
        let format = AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            encoding: AudioEncoding::Pcm,
        };
        let mut buf = AudioBuffer::new(format, 1, 1000);
        let one_sample = pcm_frame(5000, 1); // 2 bytes, 0 ms
        let mut err = None;
        for _ in 0..100_000 {
            if let Err(e) = buf.append_frame(&one_sample) {
                err = Some(e);
                break;
            }
        }
        assert!(
            matches!(err, Some(AgentError::AudioBytesExceeded { .. })),
            "a zero-duration frame flood must hit the byte cap"
        );
        assert!(buf.byte_count() <= 32_000, "buffer stayed bounded");
    }

    #[test]
    fn malformed_audio_frames_are_rejected() {
        let format = AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            encoding: AudioEncoding::Pcm,
        };
        let mut buf = AudioBuffer::new(format, 30, 1000);
        // Not a whole number of 2-byte samples.
        assert!(matches!(
            buf.append_frame(&[0u8; 3]),
            Err(AgentError::MalformedAudioFrame(_))
        ));
        // Empty frame.
        assert!(matches!(
            buf.append_frame(&[]),
            Err(AgentError::MalformedAudioFrame(_))
        ));
        // A whole-sample frame is accepted.
        assert!(buf.append_frame(&pcm_frame(5000, 160)).is_ok());
    }

    #[test]
    fn test_silence_detection() {
        // Silent frame (amplitude near zero)
        let silent = pcm_frame(10, 320);
        let format = AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            encoding: AudioEncoding::Pcm,
        };
        assert!(is_silent_frame(&silent, &format));

        // Non-silent frame
        let loud = pcm_frame(10000, 320);
        assert!(!is_silent_frame(&loud, &format));
    }

    #[test]
    fn test_take_audio() {
        let mut mgr = SttManager::new(test_config());
        let id = mgr
            .start_transcription(Uuid::new_v4(), None, Some("en".into()), false)
            .unwrap();

        let frame = pcm_frame(5000, 160);
        mgr.feed_audio_frame(&id, &frame).unwrap();

        let (audio, format, lang) = mgr.take_audio(&id).unwrap();
        assert_eq!(audio.len(), 320); // 160 samples * 2 bytes
        assert_eq!(format.sample_rate, 16000);
        assert_eq!(lang, Some("en".to_string()));
    }

    #[test]
    fn test_complete_transcription() {
        let mut mgr = SttManager::new(test_config());
        let id = mgr
            .start_transcription(Uuid::new_v4(), None, None, false)
            .unwrap();

        let result = mgr
            .complete_transcription(
                &id,
                "Hello world".into(),
                "en".into(),
                0.95,
                vec![WordTimestamp {
                    word: "Hello".into(),
                    start_ms: 0,
                    end_ms: 500,
                    confidence: 0.97,
                }],
            )
            .unwrap();

        assert_eq!(result.text, "Hello world");
        assert_eq!(result.language, "en");
        assert!(!mgr.is_active(&id));
        assert_eq!(mgr.metrics().completed, 1);
    }

    #[test]
    fn test_cancel_transcription() {
        let mut mgr = SttManager::new(test_config());
        let id = mgr
            .start_transcription(Uuid::new_v4(), None, None, false)
            .unwrap();
        assert!(mgr.cancel_transcription(&id));
        assert!(!mgr.is_active(&id));
        assert_eq!(mgr.metrics().failed, 1);
    }

    #[test]
    fn test_record_partial() {
        let mut mgr = SttManager::new(test_config());
        let id = mgr
            .start_transcription(Uuid::new_v4(), None, None, true)
            .unwrap();

        let partial = PartialTranscription {
            text: "Hell".into(),
            confidence: 0.8,
            language: Some("en".into()),
            is_final: false,
            offset_ms: 0,
            duration_ms: 300,
        };
        mgr.record_partial(&id, partial).unwrap();
        assert_eq!(mgr.partial_count(&id), Some(1));
    }

    #[test]
    fn test_validate_format_rejects_bad_sample_rate() {
        let mgr = SttManager::new(test_config());
        let format = AudioFormat {
            sample_rate: 12345,
            channels: 1,
            bit_depth: 16,
            encoding: AudioEncoding::Pcm,
        };
        let result = mgr.validate_format(&format);
        assert!(matches!(result, Err(AgentError::UnsupportedAudioFormat(_))));
    }

    #[test]
    fn test_validate_format_accepts_standard() {
        let mgr = SttManager::new(test_config());
        let format = AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            encoding: AudioEncoding::Pcm,
        };
        assert!(mgr.validate_format(&format).is_ok());
    }

    #[test]
    fn test_default_model() {
        let mgr = SttManager::new(test_config());
        assert_eq!(mgr.default_model(), "whisper-large-v3");
    }
}
