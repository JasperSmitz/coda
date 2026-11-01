//! Shared test helpers. Fixtures are generated at test time; no binary
//! files are committed.
#![allow(dead_code)]

use std::path::Path;

/// Writes a silent 16-bit mono PCM WAV of `millis` milliseconds.
pub fn write_wav(path: &Path, millis: u32) {
    std::fs::write(path, wav_bytes(millis)).expect("write wav");
}

/// A silent 16-bit mono PCM WAV of `millis` milliseconds.
pub fn wav_bytes(millis: u32) -> Vec<u8> {
    const RATE: u32 = 8000;
    let samples = RATE * millis / 1000;
    let data_len = samples * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + data_len as usize, 0);
    bytes
}

/// Writes a 16-bit PCM WAV at `rate` Hz with `channels` identical
/// channels: a sine tone of each `(hz, millis)` segment in turn, at
/// `amplitude` (0..1 of full scale). `hz` 0 is silence.
pub fn write_tones(path: &Path, rate: u32, channels: u16, segments: &[(f64, u32)], amplitude: f64) {
    let mut samples: Vec<i16> = Vec::new();
    for &(hz, millis) in segments {
        let count = u64::from(rate) * u64::from(millis) / 1000;
        for n in 0..count {
            let t = n as f64 / f64::from(rate);
            let value = amplitude * (2.0 * std::f64::consts::PI * hz * t).sin();
            let sample = (value * f64::from(i16::MAX)).round() as i16;
            for _ in 0..channels {
                samples.push(sample);
            }
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let block_align = channels * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}
