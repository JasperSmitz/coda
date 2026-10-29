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
