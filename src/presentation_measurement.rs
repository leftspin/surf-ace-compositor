//! Bounded, opt-in source evidence for display-paced drawing measurements.
//!
//! Event handlers use `try_lock`: a concurrent read-only snapshot can cause an
//! explicitly reported lost record, but can never make input or presentation
//! handling wait on the collector. The single run begins at the first primary
//! button-down and covers the approved sixty half-open one-second buckets.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

pub const MEASUREMENT_WINDOW_MICROSECONDS: u64 = 60_000_000;
pub const MEASUREMENT_BUCKET_COUNT: usize = 60;
pub const PRIMARY_BUTTON_CODE: u32 = 0x110;
const MAX_PRESENTATION_EVENTS: usize = 32_768;
const MAX_BUTTON_EVENTS: usize = 512;
const MAX_OBSERVED_OUTPUTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationMeasurementOutput {
    pub device_path: String,
    pub connector_name: String,
    pub connector_id: u32,
    pub crtc_id: u32,
    pub generation: u64,
    /// `None` means the capability query failed; only `Some(1)` aligns DRM
    /// PageFlip timestamps to CLOCK_MONOTONIC.
    pub drm_monotonic_timestamp_capability: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationMeasurementPhase {
    WaitingForPrimaryButton,
    Recording,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationButtonState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationButtonEvent {
    /// Raw libinput `time_usec` value, before any bucket conversion.
    pub timestamp_microseconds: u64,
    pub button_code: u32,
    pub state: PresentationButtonState,
    pub device_sysname: String,
    pub output_generation: Option<u64>,
    pub in_measurement_window: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPresentationEvent {
    /// Raw DRM PageFlip `tv_sec` and `tv_usec` reconstructed by drm 0.14.1.
    pub timestamp_seconds: u64,
    pub timestamp_subsecond_microseconds: u32,
    /// Normalized only when representable; the raw fields above are retained
    /// even when the value cannot be used for measurement.
    pub timestamp_microseconds: Option<u64>,
    pub crtc_id: u32,
    pub frame_sequence: u32,
    pub output_generation: Option<u64>,
    /// Duplicate deliveries are retained as raw evidence but counted once.
    pub duplicate: bool,
    pub in_measurement_window: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationMeasurementSnapshot {
    pub phase: PresentationMeasurementPhase,
    /// Capture integrity only; this does not imply the product's frame floor
    /// or human acceptance criteria passed.
    pub capture_integrity_valid: bool,
    pub invalid_reasons: Vec<String>,
    pub boot_id: Option<String>,
    pub input_clock: String,
    pub drm_clock: String,
    /// True only when the boot identity is available and the claimed DRM
    /// device reports monotonic PageFlip timestamps.
    pub clock_alignment_verified: bool,
    pub started_at_microseconds: Option<u64>,
    pub window_end_microseconds: Option<u64>,
    pub output: Option<PresentationMeasurementOutput>,
    /// Output identities in the order the runtime observed them. Event
    /// generations refer to this bounded history; `None` records loss of a
    /// claimed output. This keeps changed-output evidence inspectable even
    /// though any switch invalidates the measurement.
    pub observed_outputs: Vec<Option<PresentationMeasurementOutput>>,
    pub lost_record_count: u64,
    pub duplicate_presentation_count: u64,
    pub overflowed: bool,
    pub button_events: Vec<PresentationButtonEvent>,
    pub presentations: Vec<OutputPresentationEvent>,
    pub one_second_presentation_counts: Vec<u32>,
}

#[derive(Debug)]
struct CaptureState {
    phase: PresentationMeasurementPhase,
    invalid_reasons: Vec<&'static str>,
    started_at_microseconds: Option<u64>,
    window_end_microseconds: Option<u64>,
    output: Option<PresentationMeasurementOutput>,
    current_output: Option<PresentationMeasurementOutput>,
    observed_outputs: Vec<Option<PresentationMeasurementOutput>>,
    first_device_sysname: Option<String>,
    last_button_timestamp: Option<u64>,
    last_presentation_timestamp: Option<u64>,
    last_frame_sequence: Option<(u32, Option<u64>, u32)>,
    seen_frames: HashMap<(u32, Option<u64>, u32), (u64, u32)>,
    lost_record_count: u64,
    duplicate_presentation_count: u64,
    clock_alignment_verified: bool,
    primary_button_held: bool,
    input_window_end_observed: bool,
    presentation_window_end_observed: bool,
    overflowed: bool,
    button_events: Vec<PresentationButtonEvent>,
    presentations: Vec<OutputPresentationEvent>,
}

#[derive(Debug)]
pub struct PresentationMeasurementCollector {
    boot_id: Option<String>,
    capture: Mutex<CaptureState>,
    contended_losses: AtomicU64,
}

impl PresentationMeasurementCollector {
    /// Read the boot identity once during opt-in startup, never in an event
    /// handler. Failure is retained and makes any later run invalid.
    pub fn from_kernel_boot_id() -> Self {
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Self::new(boot_id)
    }

    pub fn new(boot_id: Option<String>) -> Self {
        Self {
            boot_id,
            capture: Mutex::new(CaptureState {
                phase: PresentationMeasurementPhase::WaitingForPrimaryButton,
                invalid_reasons: Vec::new(),
                started_at_microseconds: None,
                window_end_microseconds: None,
                output: None,
                current_output: None,
                observed_outputs: Vec::with_capacity(MAX_OBSERVED_OUTPUTS),
                first_device_sysname: None,
                last_button_timestamp: None,
                last_presentation_timestamp: None,
                last_frame_sequence: None,
                seen_frames: HashMap::with_capacity(MAX_PRESENTATION_EVENTS),
                lost_record_count: 0,
                duplicate_presentation_count: 0,
                clock_alignment_verified: false,
                primary_button_held: false,
                input_window_end_observed: false,
                presentation_window_end_observed: false,
                overflowed: false,
                button_events: Vec::with_capacity(MAX_BUTTON_EVENTS),
                presentations: Vec::with_capacity(MAX_PRESENTATION_EVENTS),
            }),
            contended_losses: AtomicU64::new(0),
        }
    }

    /// Called only when the compositor's active claimed output changes.
    pub fn set_active_output(&self, output: Option<&PresentationMeasurementOutput>) {
        let Some(mut capture) = self.try_capture() else {
            return;
        };
        let next = output.cloned();
        if capture.phase == PresentationMeasurementPhase::Recording
            && capture.current_output != next
        {
            capture.record_output_identity(next.clone());
            if capture.output != next {
                capture.invalidate("output_changed_during_measurement");
            }
        }
        capture.current_output = next;
    }

    /// Record one physical primary-button transition. The runtime filters all
    /// other input classes and button codes before invoking this method.
    pub fn record_button(
        &self,
        timestamp_microseconds: u64,
        state: PresentationButtonState,
        device_sysname: &str,
        output: Option<&PresentationMeasurementOutput>,
    ) {
        let Some(mut capture) = self.try_capture() else {
            return;
        };
        if capture.phase == PresentationMeasurementPhase::Complete {
            return;
        }

        if capture.phase == PresentationMeasurementPhase::WaitingForPrimaryButton {
            if state != PresentationButtonState::Pressed {
                return;
            }
            capture.phase = PresentationMeasurementPhase::Recording;
            capture.started_at_microseconds = Some(timestamp_microseconds);
            capture.window_end_microseconds =
                timestamp_microseconds.checked_add(MEASUREMENT_WINDOW_MICROSECONDS);
            capture.output = output.cloned();
            capture.record_output_identity(output.cloned());
            capture.first_device_sysname = Some(device_sysname.to_owned());
            capture.primary_button_held = true;
            if device_sysname.is_empty() {
                capture.invalidate("primary_button_device_identity_unavailable");
            }
            capture.clock_alignment_verified = self.boot_id.is_some()
                && output
                    .is_some_and(|identity| identity.drm_monotonic_timestamp_capability == Some(1));
            if self.boot_id.is_none() {
                capture.invalidate("boot_id_unavailable");
            }
            match output {
                None => capture.invalidate("claimed_output_unavailable_at_start"),
                Some(identity) if identity.drm_monotonic_timestamp_capability != Some(1) => {
                    capture.invalidate("drm_monotonic_timestamp_unavailable")
                }
                Some(identity) if identity.crtc_id == 0 => {
                    capture.invalidate("claimed_crtc_unavailable_at_start")
                }
                Some(_) => {}
            }
            if capture.current_output.as_ref() != output {
                capture.invalidate("claimed_output_identity_unstable_at_start");
            }
            if capture.window_end_microseconds.is_none() {
                capture.invalidate("measurement_window_timestamp_overflow");
            }
        } else {
            capture.check_current_output(output);
            match (capture.primary_button_held, state) {
                (true, PresentationButtonState::Pressed) => {
                    capture.invalidate("primary_button_pressed_without_release");
                }
                (false, PresentationButtonState::Released) => {
                    capture.invalidate("primary_button_released_without_press");
                }
                (false, PresentationButtonState::Pressed) => {
                    capture.primary_button_held = true;
                }
                (true, PresentationButtonState::Released) => {
                    capture.primary_button_held = false;
                }
            }
        }

        if capture
            .last_button_timestamp
            .is_some_and(|last| timestamp_microseconds < last)
        {
            capture.invalidate("non_monotonic_input_timestamp");
        }
        capture.last_button_timestamp = Some(timestamp_microseconds);
        if capture
            .first_device_sysname
            .as_deref()
            .is_some_and(|first| first != device_sysname)
        {
            capture.invalidate("primary_button_device_changed");
        }

        let started_at = capture
            .started_at_microseconds
            .unwrap_or(timestamp_microseconds);
        let end = capture.window_end_microseconds;
        let in_measurement_window = end.is_some_and(|end| {
            timestamp_microseconds >= started_at && timestamp_microseconds < end
        });
        capture.push_button(PresentationButtonEvent {
            timestamp_microseconds,
            button_code: PRIMARY_BUTTON_CODE,
            state,
            device_sysname: device_sysname.to_owned(),
            output_generation: output.map(|identity| identity.generation),
            in_measurement_window,
        });

        if state == PresentationButtonState::Released
            && end.is_some_and(|end| timestamp_microseconds < end)
        {
            capture.invalidate("primary_button_released_before_window_end");
        }
        if end.is_some_and(|end| timestamp_microseconds >= end) {
            capture.input_window_end_observed = true;
            capture.finish_if_both_streams_passed_window_end();
        }
    }

    /// A removed physical input device cannot prove an uninterrupted primary
    /// button hold. Only the device that started the measurement matters.
    pub fn record_input_device_removed(&self, device_sysname: &str) {
        let Some(mut capture) = self.try_capture() else {
            return;
        };
        if capture.phase == PresentationMeasurementPhase::Recording
            && capture.first_device_sysname.as_deref() == Some(device_sysname)
        {
            capture.invalidate("primary_button_device_removed_during_measurement");
        }
    }

    /// Record only after the production pipeline accepts a PageFlip for its
    /// claimed CRTC while a flip is pending.
    pub fn record_presentation(
        &self,
        timestamp: Duration,
        crtc_id: u32,
        frame_sequence: u32,
        output: Option<&PresentationMeasurementOutput>,
    ) {
        let Some(mut capture) = self.try_capture() else {
            return;
        };
        if matches!(
            capture.phase,
            PresentationMeasurementPhase::WaitingForPrimaryButton
                | PresentationMeasurementPhase::Complete
        ) {
            return;
        }
        capture.check_current_output(output);

        let timestamp_seconds = timestamp.as_secs();
        let timestamp_subsecond_microseconds = timestamp.subsec_micros();
        let timestamp_microseconds = u64::try_from(timestamp.as_micros()).ok();
        if timestamp_microseconds.is_none() {
            capture.invalidate("presentation_timestamp_unrepresentable");
        }
        if output.is_none_or(|identity| identity.crtc_id != crtc_id) {
            capture.invalidate("presentation_crtc_does_not_match_claimed_output");
        }

        let output_generation = output.map(|identity| identity.generation);
        let key = (crtc_id, output_generation, frame_sequence);
        let duplicate =
            if let Some((prior_seconds, prior_subsecond)) = capture.seen_frames.get(&key) {
                capture.duplicate_presentation_count =
                    capture.duplicate_presentation_count.saturating_add(1);
                if *prior_seconds != timestamp_seconds
                    || *prior_subsecond != timestamp_subsecond_microseconds
                {
                    capture.invalidate("duplicate_frame_timestamp_conflict");
                }
                true
            } else {
                if capture.seen_frames.len() == MAX_PRESENTATION_EVENTS {
                    capture.mark_lost_record("presentation_event_buffer_overflow");
                    return;
                }
                capture
                    .seen_frames
                    .insert(key, (timestamp_seconds, timestamp_subsecond_microseconds));
                if capture
                    .last_presentation_timestamp
                    .zip(timestamp_microseconds)
                    .is_some_and(|(last, current)| current < last)
                {
                    capture.invalidate("non_monotonic_presentation_timestamp");
                }
                if let Some(current) = timestamp_microseconds {
                    capture.last_presentation_timestamp = Some(current);
                }
                if capture.last_frame_sequence.is_some_and(
                    |(last_crtc, last_generation, last_sequence)| {
                        last_crtc == crtc_id
                            && last_generation == output_generation
                            && frame_sequence < last_sequence
                    },
                ) {
                    capture.invalidate("frame_sequence_wrap_or_regression");
                }
                capture.last_frame_sequence = Some((crtc_id, output_generation, frame_sequence));
                false
            };

        let started_at = capture.started_at_microseconds;
        let end = capture.window_end_microseconds;
        let in_measurement_window = match (started_at, end, timestamp_microseconds) {
            (Some(start), Some(end), Some(current)) => current >= start && current < end,
            _ => false,
        };
        capture.push_presentation(OutputPresentationEvent {
            timestamp_seconds,
            timestamp_subsecond_microseconds,
            timestamp_microseconds,
            crtc_id,
            frame_sequence,
            output_generation,
            duplicate,
            in_measurement_window,
        });

        if end.is_some_and(|end| timestamp_microseconds.is_some_and(|current| current >= end)) {
            capture.presentation_window_end_observed = true;
            capture.finish_if_both_streams_passed_window_end();
        }
    }

    /// Snapshot is a bounded read-only copy. Event handlers never wait for it:
    /// their try-lock records an explicit loss if this copy is in progress.
    pub fn snapshot(&self) -> PresentationMeasurementSnapshot {
        let capture = match self.capture.lock() {
            Ok(capture) => capture,
            Err(poisoned) => poisoned.into_inner(),
        };
        let contended_losses = self.contended_losses.load(Ordering::Relaxed);
        let lost_record_count = capture.lost_record_count.saturating_add(contended_losses);
        let one_second_presentation_counts = capture.bucket_counts();
        let capture_integrity_valid = capture.phase == PresentationMeasurementPhase::Complete
            && capture.invalid_reasons.is_empty()
            && lost_record_count == 0
            && !capture.overflowed;
        let mut invalid_reasons = capture
            .invalid_reasons
            .iter()
            .map(|reason| (*reason).to_string())
            .collect::<Vec<_>>();
        if contended_losses > 0
            && !invalid_reasons
                .iter()
                .any(|reason| reason == "collector_lock_contention")
        {
            invalid_reasons.push("collector_lock_contention".to_string());
        }
        PresentationMeasurementSnapshot {
            phase: capture.phase,
            capture_integrity_valid,
            invalid_reasons,
            boot_id: self.boot_id.clone(),
            input_clock: "libinput event time_usec (CLOCK_MONOTONIC)".to_string(),
            drm_clock: "DRM PageFlip event timestamp (CLOCK_MONOTONIC only when capability=1)"
                .to_string(),
            clock_alignment_verified: capture.clock_alignment_verified,
            started_at_microseconds: capture.started_at_microseconds,
            window_end_microseconds: capture.window_end_microseconds,
            output: capture.output.clone(),
            observed_outputs: capture.observed_outputs.clone(),
            lost_record_count,
            duplicate_presentation_count: capture.duplicate_presentation_count,
            overflowed: capture.overflowed,
            button_events: capture.button_events.clone(),
            presentations: capture.presentations.clone(),
            one_second_presentation_counts,
        }
    }

    fn try_capture(&self) -> Option<std::sync::MutexGuard<'_, CaptureState>> {
        match self.capture.try_lock() {
            Ok(capture) => Some(capture),
            Err(TryLockError::WouldBlock) => {
                self.record_contended_loss();
                None
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                self.record_contended_loss();
                let mut capture = poisoned.into_inner();
                capture.invalidate("collector_state_poisoned");
                None
            }
        }
    }

    fn record_contended_loss(&self) {
        let _ = self
            .contended_losses
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            });
    }
}

impl CaptureState {
    fn check_current_output(&mut self, output: Option<&PresentationMeasurementOutput>) {
        if self.phase != PresentationMeasurementPhase::Recording {
            return;
        }
        let next = output.cloned();
        if self.current_output != next {
            self.record_output_identity(next.clone());
            self.current_output = next;
        }
        if self.output.as_ref() != output {
            self.invalidate("output_changed_during_measurement");
        }
    }

    fn finish_if_both_streams_passed_window_end(&mut self) {
        if self.phase == PresentationMeasurementPhase::Recording
            && self.input_window_end_observed
            && self.presentation_window_end_observed
        {
            self.phase = PresentationMeasurementPhase::Complete;
        }
    }

    fn record_output_identity(&mut self, output: Option<PresentationMeasurementOutput>) {
        if self.observed_outputs.len() == MAX_OBSERVED_OUTPUTS {
            self.mark_lost_record("output_identity_history_overflow");
        } else {
            self.observed_outputs.push(output);
        }
    }

    fn invalidate(&mut self, reason: &'static str) {
        if !self.invalid_reasons.contains(&reason) {
            self.invalid_reasons.push(reason);
        }
    }

    fn push_button(&mut self, event: PresentationButtonEvent) {
        if self.button_events.len() == MAX_BUTTON_EVENTS {
            self.mark_lost_record("button_event_buffer_overflow");
            return;
        }
        self.button_events.push(event);
    }

    fn push_presentation(&mut self, event: OutputPresentationEvent) {
        if self.presentations.len() == MAX_PRESENTATION_EVENTS {
            self.mark_lost_record("presentation_event_buffer_overflow");
            return;
        }
        self.presentations.push(event);
    }

    fn mark_lost_record(&mut self, reason: &'static str) {
        self.overflowed = true;
        self.lost_record_count = self.lost_record_count.saturating_add(1);
        self.invalidate(reason);
    }

    fn bucket_counts(&self) -> Vec<u32> {
        let mut counts = vec![0; MEASUREMENT_BUCKET_COUNT];
        let Some(start) = self.started_at_microseconds else {
            return counts;
        };
        let Some(end) = self.window_end_microseconds else {
            return counts;
        };
        for event in &self.presentations {
            let Some(timestamp) = event.timestamp_microseconds else {
                continue;
            };
            if event.duplicate
                || !event.in_measurement_window
                || timestamp < start
                || timestamp >= end
            {
                continue;
            }
            let bucket = ((timestamp - start) / 1_000_000) as usize;
            if let Some(count) = counts.get_mut(bucket) {
                *count = count.saturating_add(1);
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureState, MAX_BUTTON_EVENTS, MAX_PRESENTATION_EVENTS, MEASUREMENT_WINDOW_MICROSECONDS,
        PresentationButtonState, PresentationMeasurementCollector, PresentationMeasurementOutput,
        PresentationMeasurementPhase,
    };
    use std::collections::HashMap;
    use std::time::Duration;

    fn output(generation: u64, monotonic: Option<u64>) -> PresentationMeasurementOutput {
        PresentationMeasurementOutput {
            device_path: "/dev/dri/card-test".to_string(),
            connector_name: "HDMI-A-1".to_string(),
            connector_id: 10,
            crtc_id: 20,
            generation,
            drm_monotonic_timestamp_capability: monotonic,
        }
    }

    fn collector() -> PresentationMeasurementCollector {
        PresentationMeasurementCollector::new(Some("boot-test".to_string()))
    }

    fn start(capture: &PresentationMeasurementCollector, output: &PresentationMeasurementOutput) {
        capture.set_active_output(Some(output));
        capture.record_button(
            10_000_000,
            PresentationButtonState::Pressed,
            "event-test",
            Some(output),
        );
    }

    #[test]
    fn deduplicates_the_same_crtc_and_frame_without_counting_it_twice() {
        let capture = collector();
        let output = output(1, Some(1));
        start(&capture, &output);
        for (timestamp, frame) in [(10_100_000, 1), (10_100_000, 1), (10_200_000, 2)] {
            capture.record_presentation(Duration::from_micros(timestamp), 20, frame, Some(&output));
        }
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.presentations.len(), 3);
        assert_eq!(snapshot.duplicate_presentation_count, 1);
        assert_eq!(snapshot.one_second_presentation_counts[0], 2);
        assert!(snapshot.presentations[1].duplicate);
    }

    #[test]
    fn frame_sequence_reuse_on_a_new_output_generation_is_not_a_duplicate() {
        let capture = collector();
        let first = output(1, Some(1));
        let reclaimed = output(2, Some(1));
        start(&capture, &first);
        capture.record_presentation(Duration::from_micros(10_100_000), 20, 7, Some(&first));
        capture.set_active_output(Some(&reclaimed));
        capture.record_presentation(Duration::from_micros(10_200_000), 20, 7, Some(&reclaimed));

        let snapshot = capture.snapshot();
        assert_eq!(snapshot.presentations.len(), 2);
        assert!(!snapshot.presentations[0].duplicate);
        assert!(!snapshot.presentations[1].duplicate);
        assert_eq!(snapshot.presentations[0].output_generation, Some(1));
        assert_eq!(snapshot.presentations[1].output_generation, Some(2));
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"output_changed_during_measurement".to_string())
        );
    }

    #[test]
    fn event_path_preserves_new_output_identity_if_claim_update_contends() {
        let capture = collector();
        let first = output(1, Some(1));
        let second = output(2, Some(1));
        start(&capture, &first);
        {
            let _guard = capture.capture.lock().unwrap();
            capture.set_active_output(Some(&second));
        }
        capture.record_presentation(Duration::from_micros(10_100_000), 20, 8, Some(&second));

        let snapshot = capture.snapshot();
        assert_eq!(snapshot.observed_outputs, vec![Some(first), Some(second)]);
        assert_eq!(snapshot.lost_record_count, 1);
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"collector_lock_contention".to_string())
        );
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"output_changed_during_measurement".to_string())
        );
    }

    #[test]
    fn uses_half_open_one_second_buckets_and_excludes_the_sixtieth_second_endpoint() {
        let capture = collector();
        let output = output(1, Some(1));
        start(&capture, &output);
        for (index, timestamp) in [10_000_000, 10_999_999, 11_000_000, 69_999_999, 70_000_000]
            .into_iter()
            .enumerate()
        {
            capture.record_presentation(
                Duration::from_micros(timestamp),
                20,
                index as u32 + 1,
                Some(&output),
            );
        }
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.one_second_presentation_counts[0], 2);
        assert_eq!(snapshot.one_second_presentation_counts[1], 1);
        assert_eq!(snapshot.one_second_presentation_counts[59], 1);
        assert!(!snapshot.presentations.last().unwrap().in_measurement_window);
        assert_eq!(
            snapshot.one_second_presentation_counts.iter().sum::<u32>(),
            4
        );
        assert_eq!(
            snapshot.window_end_microseconds,
            Some(10_000_000 + MEASUREMENT_WINDOW_MICROSECONDS)
        );
        assert_eq!(snapshot.phase, PresentationMeasurementPhase::Recording);
    }

    #[test]
    fn completes_only_after_input_and_presentation_streams_pass_the_window_end() {
        let output = output(1, Some(1));
        let end = 10_000_000 + MEASUREMENT_WINDOW_MICROSECONDS;

        let input_first = collector();
        start(&input_first, &output);
        input_first.record_button(
            end,
            PresentationButtonState::Released,
            "event-test",
            Some(&output),
        );
        assert_eq!(
            input_first.snapshot().phase,
            PresentationMeasurementPhase::Recording
        );
        input_first.record_presentation(Duration::from_micros(end - 1), 20, 1, Some(&output));
        input_first.record_presentation(Duration::from_micros(end), 20, 2, Some(&output));
        let input_first_snapshot = input_first.snapshot();
        assert_eq!(
            input_first_snapshot.phase,
            PresentationMeasurementPhase::Complete
        );
        assert_eq!(input_first_snapshot.one_second_presentation_counts[59], 1);
        assert!(input_first_snapshot.capture_integrity_valid);

        let presentation_first = collector();
        start(&presentation_first, &output);
        presentation_first.record_presentation(Duration::from_micros(end), 20, 3, Some(&output));
        assert_eq!(
            presentation_first.snapshot().phase,
            PresentationMeasurementPhase::Recording
        );
        presentation_first.record_button(
            end,
            PresentationButtonState::Released,
            "event-test",
            Some(&output),
        );
        assert_eq!(
            presentation_first.snapshot().phase,
            PresentationMeasurementPhase::Complete
        );
    }

    #[test]
    fn refuses_missing_boot_or_unavailable_monotonic_clock_capability() {
        let no_boot = PresentationMeasurementCollector::new(None);
        let output = output(1, Some(1));
        start(&no_boot, &output);
        assert!(
            no_boot
                .snapshot()
                .invalid_reasons
                .contains(&"boot_id_unavailable".to_string())
        );

        let no_clock = collector();
        let output = output(1, Some(0));
        start(&no_clock, &output);
        let snapshot = no_clock.snapshot();
        assert!(!snapshot.clock_alignment_verified);
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"drm_monotonic_timestamp_unavailable".to_string())
        );

        let unknown_clock = collector();
        let output = output(1, None);
        start(&unknown_clock, &output);
        assert!(
            unknown_clock
                .snapshot()
                .invalid_reasons
                .contains(&"drm_monotonic_timestamp_unavailable".to_string())
        );
    }

    #[test]
    fn refuses_a_missing_or_lost_claimed_output() {
        let missing_output = collector();
        missing_output.record_button(
            10_000_000,
            PresentationButtonState::Pressed,
            "event-test",
            None,
        );
        assert!(
            missing_output
                .snapshot()
                .invalid_reasons
                .contains(&"claimed_output_unavailable_at_start".to_string())
        );

        let lost_output = collector();
        let output = output(1, Some(1));
        start(&lost_output, &output);
        lost_output.set_active_output(None);
        let snapshot = lost_output.snapshot();
        assert_eq!(snapshot.observed_outputs, vec![Some(output), None]);
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"output_changed_during_measurement".to_string())
        );
    }

    #[test]
    fn marks_output_change_device_change_and_timestamp_regression_invalid() {
        let capture = collector();
        let first = output(1, Some(1));
        let second = output(2, Some(1));
        start(&capture, &first);
        capture.set_active_output(Some(&second));
        capture.record_button(
            9_999_999,
            PresentationButtonState::Pressed,
            "other-device",
            Some(&second),
        );
        let reasons = capture.snapshot().invalid_reasons;
        assert!(reasons.contains(&"output_changed_during_measurement".to_string()));
        assert!(reasons.contains(&"non_monotonic_input_timestamp".to_string()));
        assert!(reasons.contains(&"primary_button_device_changed".to_string()));
        assert_eq!(
            capture.snapshot().observed_outputs,
            vec![Some(first), Some(second)]
        );
    }

    #[test]
    fn refuses_removal_of_the_primary_button_device_during_measurement() {
        let capture = collector();
        let output = output(1, Some(1));
        start(&capture, &output);
        capture.record_input_device_removed("unrelated-event-device");
        assert!(
            !capture
                .snapshot()
                .invalid_reasons
                .contains(&"primary_button_device_removed_during_measurement".to_string())
        );

        capture.record_input_device_removed("event-test");
        assert!(
            capture
                .snapshot()
                .invalid_reasons
                .contains(&"primary_button_device_removed_during_measurement".to_string())
        );
    }

    #[test]
    fn refuses_pageflip_sequence_wrap_and_released_stroke() {
        let capture = collector();
        let output = output(1, Some(1));
        start(&capture, &output);
        capture.record_presentation(
            Duration::from_micros(10_100_000),
            20,
            u32::MAX,
            Some(&output),
        );
        capture.record_presentation(Duration::from_micros(10_200_000), 20, 0, Some(&output));
        capture.record_button(
            11_000_000,
            PresentationButtonState::Released,
            "event-test",
            Some(&output),
        );
        let reasons = capture.snapshot().invalid_reasons;
        assert!(reasons.contains(&"frame_sequence_wrap_or_regression".to_string()));
        assert!(reasons.contains(&"primary_button_released_before_window_end".to_string()));
    }

    #[test]
    fn refuses_repeated_primary_press_during_the_continuous_stroke() {
        let capture = collector();
        let output = output(1, Some(1));
        start(&capture, &output);
        capture.record_button(
            10_100_000,
            PresentationButtonState::Pressed,
            "event-test",
            Some(&output),
        );

        assert!(
            capture
                .snapshot()
                .invalid_reasons
                .contains(&"primary_button_pressed_without_release".to_string())
        );
    }

    #[test]
    fn reports_lock_contention_and_capacity_overflow_without_growing_buffers() {
        let capture = collector();
        {
            let _guard = capture.capture.lock().unwrap();
            capture.record_button(
                10_000_000,
                PresentationButtonState::Pressed,
                "event-test",
                Some(&output(1, Some(1))),
            );
        }
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.lost_record_count, 1);
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"collector_lock_contention".to_string())
        );

        let mut state = CaptureState {
            phase: PresentationMeasurementPhase::WaitingForPrimaryButton,
            invalid_reasons: Vec::new(),
            started_at_microseconds: None,
            window_end_microseconds: None,
            output: None,
            current_output: None,
            observed_outputs: Vec::with_capacity(super::MAX_OBSERVED_OUTPUTS),
            first_device_sysname: None,
            last_button_timestamp: None,
            last_presentation_timestamp: None,
            last_frame_sequence: None,
            seen_frames: HashMap::with_capacity(MAX_PRESENTATION_EVENTS),
            lost_record_count: 0,
            duplicate_presentation_count: 0,
            clock_alignment_verified: false,
            primary_button_held: false,
            input_window_end_observed: false,
            presentation_window_end_observed: false,
            overflowed: false,
            button_events: Vec::with_capacity(MAX_BUTTON_EVENTS),
            presentations: Vec::with_capacity(MAX_PRESENTATION_EVENTS),
        };
        for _ in 0..MAX_PRESENTATION_EVENTS {
            state.push_presentation(super::OutputPresentationEvent {
                timestamp_seconds: 1,
                timestamp_subsecond_microseconds: 0,
                timestamp_microseconds: Some(1_000_000),
                crtc_id: 20,
                frame_sequence: 1,
                output_generation: Some(1),
                duplicate: false,
                in_measurement_window: true,
            });
        }
        state.push_presentation(super::OutputPresentationEvent {
            timestamp_seconds: 1,
            timestamp_subsecond_microseconds: 1,
            timestamp_microseconds: Some(1_000_001),
            crtc_id: 20,
            frame_sequence: 2,
            output_generation: Some(1),
            duplicate: false,
            in_measurement_window: true,
        });
        assert!(state.overflowed);
        assert_eq!(state.lost_record_count, 1);
        assert!(
            state
                .invalid_reasons
                .contains(&"presentation_event_buffer_overflow")
        );

        let mut buttons = CaptureState {
            phase: PresentationMeasurementPhase::Recording,
            invalid_reasons: Vec::new(),
            started_at_microseconds: Some(1),
            window_end_microseconds: Some(MEASUREMENT_WINDOW_MICROSECONDS + 1),
            output: Some(output(1, Some(1))),
            current_output: Some(output(1, Some(1))),
            observed_outputs: vec![Some(output(1, Some(1)))],
            first_device_sysname: Some("event-test".to_string()),
            last_button_timestamp: None,
            last_presentation_timestamp: None,
            last_frame_sequence: None,
            seen_frames: HashMap::with_capacity(MAX_PRESENTATION_EVENTS),
            lost_record_count: 0,
            duplicate_presentation_count: 0,
            clock_alignment_verified: true,
            primary_button_held: true,
            input_window_end_observed: false,
            presentation_window_end_observed: false,
            overflowed: false,
            button_events: Vec::with_capacity(MAX_BUTTON_EVENTS),
            presentations: Vec::with_capacity(MAX_PRESENTATION_EVENTS),
        };
        for _ in 0..MAX_BUTTON_EVENTS {
            buttons.push_button(super::PresentationButtonEvent {
                timestamp_microseconds: 1,
                button_code: super::PRIMARY_BUTTON_CODE,
                state: PresentationButtonState::Pressed,
                device_sysname: "event-test".to_string(),
                output_generation: Some(1),
                in_measurement_window: true,
            });
        }
        buttons.push_button(super::PresentationButtonEvent {
            timestamp_microseconds: 2,
            button_code: super::PRIMARY_BUTTON_CODE,
            state: PresentationButtonState::Released,
            device_sysname: "event-test".to_string(),
            output_generation: Some(1),
            in_measurement_window: true,
        });
        assert!(buttons.overflowed);
        assert_eq!(buttons.lost_record_count, 1);
        assert!(
            buttons
                .invalid_reasons
                .contains(&"button_event_buffer_overflow")
        );

        let output_history = collector();
        let first_output = output(1, Some(1));
        start(&output_history, &first_output);
        for generation in 2..=super::MAX_OBSERVED_OUTPUTS as u64 + 1 {
            output_history.set_active_output(Some(&output(generation, Some(1))));
        }
        let snapshot = output_history.snapshot();
        assert!(snapshot.overflowed);
        assert_eq!(snapshot.lost_record_count, 1);
        assert!(
            snapshot
                .invalid_reasons
                .contains(&"output_identity_history_overflow".to_string())
        );
        assert_eq!(snapshot.observed_outputs.len(), super::MAX_OBSERVED_OUTPUTS);
    }

    #[test]
    fn marks_window_end_overflow_and_preserves_recorded_pageflips_across_vblank_skips() {
        let overflow = collector();
        let output = output(1, Some(1));
        overflow.set_active_output(Some(&output));
        overflow.record_button(
            u64::MAX - 10,
            PresentationButtonState::Pressed,
            "event-test",
            Some(&output),
        );
        assert!(
            overflow
                .snapshot()
                .invalid_reasons
                .contains(&"measurement_window_timestamp_overflow".to_string())
        );

        let capture = collector();
        start(&capture, &output);
        capture.record_presentation(Duration::from_micros(10_100_000), 20, 1, Some(&output));
        capture.record_presentation(Duration::from_micros(10_200_000), 20, 4, Some(&output));
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.one_second_presentation_counts[0], 2);
        assert_eq!(snapshot.lost_record_count, 0);
        assert_eq!(
            snapshot
                .presentations
                .iter()
                .map(|event| event.frame_sequence)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
    }
}
