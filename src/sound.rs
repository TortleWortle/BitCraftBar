//! Alert sounds (settings → Sounds). Short chimes synthesized in code (no
//! sound files), played with rodio on a background thread.

use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

const SAMPLE_RATE: u32 = 44_100;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sound {
    /// An active craft completed: rising two-note chime.
    CraftFinished,
    /// Food regen buff ran out: falling two-note chime, twice.
    OutOfFood,
    /// Idle with stamina above 90%: three soft pulses.
    IdleFull,
}

impl Sound {
    /// (frequency Hz, start s, length s) per note.
    fn notes(self) -> &'static [(f32, f32, f32)] {
        match self {
            Sound::CraftFinished => &[(659.3, 0.0, 0.35), (987.8, 0.12, 0.5)],
            Sound::OutOfFood => &[(784.0, 0.0, 0.25), (523.3, 0.14, 0.35), (784.0, 0.5, 0.25), (523.3, 0.64, 0.4)],
            Sound::IdleFull => &[(587.3, 0.0, 0.18), (587.3, 0.25, 0.18), (880.0, 0.5, 0.35)],
        }
    }

    /// Mono samples: sine notes with a quick attack and exponential decay,
    /// plus a quiet octave for a bell-like tone.
    fn samples(self) -> Vec<f32> {
        let notes = self.notes();
        let end = notes.iter().map(|(_, s, l)| s + l).fold(0.0, f32::max);
        let len = (end * SAMPLE_RATE as f32) as usize;
        let mut out = vec![0.0f32; len];
        for &(freq, start, length) in notes {
            let first = (start * SAMPLE_RATE as f32) as usize;
            let count = (length * SAMPLE_RATE as f32) as usize;
            for i in 0..count.min(len.saturating_sub(first)) {
                let t = i as f32 / SAMPLE_RATE as f32;
                let attack = (t / 0.005).min(1.0);
                let decay = (-t * 6.0 / length).exp();
                let phase = 2.0 * std::f32::consts::PI * freq * t;
                out[first + i] += 0.35 * attack * decay * (phase.sin() + 0.25 * (2.0 * phase).sin());
            }
        }
        out
    }
}

/// Play `sound` at `volume` (0–1) without blocking. Errors (no audio
/// device) are logged and otherwise ignored.
pub fn play(sound: Sound, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    std::thread::spawn(move || {
        let (_stream, handle) = match OutputStream::try_default() {
            Ok(s) => s,
            Err(e) => return eprintln!("sound: {e}"),
        };
        let sink = match Sink::try_new(&handle) {
            Ok(s) => s,
            Err(e) => return eprintln!("sound: {e}"),
        };
        sink.set_volume(volume.clamp(0.0, 1.0));
        sink.append(SamplesBuffer::new(1, SAMPLE_RATE, sound.samples()));
        sink.sleep_until_end();
        // Let the device drain the last buffer before the stream drops.
        std::thread::sleep(Duration::from_millis(100));
    });
}
