//! cpal audio output fed from the emulator's sample queue.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Max buffered stereo samples (~170 ms at 48 kHz) before old ones are dropped.
const MAX_QUEUE: usize = 16_384;

pub struct AudioOut {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<f32>>>,
    device_rate: u32,
    /// Fractional read position for linear resampling.
    resample_pos: f64,
    last_frame: [f32; 2],
}

impl AudioOut {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let device_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let q = queue.clone();
        let stream = device
            .build_output_stream(
                config.config(),
                move |out: &mut [f32], _| {
                    let mut q = q.lock().unwrap();
                    for frame in out.chunks_mut(channels) {
                        let l = q.pop_front().unwrap_or(0.0);
                        let r = q.pop_front().unwrap_or(0.0);
                        for (i, s) in frame.iter_mut().enumerate() {
                            *s = if i % 2 == 0 { l } else { r };
                        }
                    }
                },
                |e| log::warn!("audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(AudioOut {
            _stream: stream,
            queue,
            device_rate,
            resample_pos: 0.0,
            last_frame: [0.0; 2],
        })
    }

    /// Queue interleaved stereo samples produced at `src_rate`.
    pub fn push(&mut self, samples: &[f32], src_rate: u32) {
        if samples.len() < 2 {
            return;
        }
        let mut q = self.queue.lock().unwrap();
        if self.device_rate == src_rate {
            q.extend(samples.iter().copied());
        } else {
            let ratio = src_rate as f64 / self.device_rate as f64;
            let frames = samples.len() / 2;
            let mut pos = self.resample_pos;
            while (pos as usize) < frames {
                let i = pos as usize;
                let frac = (pos - i as f64) as f32;
                let (l0, r0) = if i == 0 {
                    (self.last_frame[0], self.last_frame[1])
                } else {
                    (samples[(i - 1) * 2], samples[(i - 1) * 2 + 1])
                };
                let l1 = samples[i * 2];
                let r1 = samples[i * 2 + 1];
                q.push_back(l0 + (l1 - l0) * frac);
                q.push_back(r0 + (r1 - r0) * frac);
                pos += ratio;
            }
            self.resample_pos = pos - frames as f64;
            self.last_frame = [samples[(frames - 1) * 2], samples[(frames - 1) * 2 + 1]];
        }
        let excess = q.len().saturating_sub(MAX_QUEUE);
        if excess > 0 {
            q.drain(..excess);
        }
    }

}
