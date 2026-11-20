// speak.rs
// Phonon (libptts) runner with persian_normalizer, tokenizer, RMS silence trimming,
// fade-in/out, 16-bit 24 kHz PCM output, and optional word-level timing JSON.

use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use persian_normalizer::normalize_with_dicts;
use tokenizer::Tokenizer;
use std::slice;

const SAMPLE_RATE: usize = 24000;
const PAUSE_MS: usize = 350;
const DEFAULT_MODEL_DIR: &str = "./model";
const DEFAULT_DICT_DIR: &str = "./dict";
const DEFAULT_LANG: &str = "none";
const DEFAULT_VOICE: &str = "voice1";
const DEFAULT_TEMPERATURE: f32 = 0.3;
const DEFAULT_SEED: u64 = 0;

const TRIM_WINDOW_MS: usize = 20;
const TRIM_KEEP_MS: usize = 60;
const TRIM_THRESHOLD: f64 = 0.10;
const CLEAN_MUTE_MS: usize = 0;
const CLEAN_FADE_MS: usize = 20;

const TIMING_WINDOW_MS: usize = 20;
const TIMING_HOP_MS: usize = 10;
const TIMING_MIN_GAP: f64 = 0.05;
const TIMING_SEARCH_MS: usize = 80;

// Match enum values in ptts.h
#[cfg(target_os = "macos")]
const PTTS_UNIT_DEFAULT: u32 = 1; // PTTS_UNIT_ANE
#[cfg(not(target_os = "macos"))]
const PTTS_UNIT_DEFAULT: u32 = 0; // PTTS_UNIT_CPU

#[repr(C)]
struct PttsHandle {
    _private: [u8; 0],
}

// Generous aligned buffer for PttsResult out-parameter from ptts.h
#[repr(C)]
struct PttsResult {
    _opaque: [u64; 16],
}

type PttsAudioCb = extern "C" fn(pcm: *const f32, n: usize, user: *mut c_void) -> bool;

extern "C" {
    fn ptts_new(model: *const c_char, unit: u32, lang: *const c_char) -> *mut PttsHandle;
    fn ptts_free(h: *mut PttsHandle);
    fn ptts_set_voice(h: *mut PttsHandle, voice: *const c_char) -> bool;
    fn ptts_voices(h: *mut PttsHandle) -> *const c_char;
    fn ptts_set_temperature(h: *mut PttsHandle, temp: f32) -> bool;
    fn ptts_set_seed(h: *mut PttsHandle, seed: u64);
    fn ptts_speak(
        h: *mut PttsHandle,
        text: *const c_char,
        cb: PttsAudioCb,
        user: *mut c_void,
        result: *mut PttsResult,
    ) -> bool;
    fn ptts_last_error(h: *mut PttsHandle) -> *const c_char;
}

// ---------------------------------------------------------------------
// Help & Path Resolution
// ---------------------------------------------------------------------

fn print_help(prog: &str) {
    println!(
        r#"Persian TTS runner (Phonon / libptts) — outputs 24 kHz 16-bit signed LE mono PCM to stdout.

USAGE:
  {prog} TEXT
  echo "TEXT" | {prog}
  {prog} -h | --help

INPUT:
  TEXT          Text to synthesize. If omitted (or "-"), reads UTF-8 text from stdin.

DEFAULT LAYOUT:
  ./model/                  Model files, tokenizer.json, timing_model.json
  ./dict/                   cmudict.txt, latin_dict.txt
  ./persian_normalizer      Text normalizer binary
  ./tokenizer               Sentence chunker / tokenizer binary

ENVIRONMENT VARIABLES (all optional, override defaults before running):
  Model & Synthesis:
    PTTS_MODEL              Model directory path (default: ./model)
    PTTS_LANG               Language code (default: none)
    PTTS_VOICE              Voice name (default: voice1)
    PTTS_TEMPERATURE        Sampling temperature, float (default: {temp})
    PTTS_SEED               RNG seed, unsigned integer (default: {seed})

  Model Assets (auto-detected inside PTTS_MODEL directory):
    TOKENIZER_JSON          Path to tokenizer.json (default: <PTTS_MODEL>/tokenizer.json)
    SPEAK_TIMING            Output path for word-level timing JSON (disabled if unset)
    SPEAK_TIMING_MODEL      Path to timing_model.json (default: <PTTS_MODEL>/timing_model.json)

  Dictionaries (passed to persian_normalizer):
    DICT_DIR                Dictionary directory (default: ./dict)
    G2P_CMUDICT             Path to cmudict.txt (default: <DICT_DIR>/cmudict.txt)
    G2P_LATIN_DICT          Path to latin_dict.txt (default: <DICT_DIR>/latin_dict.txt)

  Helper Binaries:
    PERSIAN_NORMALIZER      Path to persian_normalizer (default: ./persian_normalizer)
    TOKENIZER               Path to tokenizer (default: ./tokenizer)

EXAMPLES:
  # Direct text:
  {prog} "سلام دنیا" | pacat --raw --format=s16le --rate=24000 --channels=1

  # Pipe from Termux clipboard:
  termux-clipboard-get | {prog} | pacat --raw --format=s16le --rate=24000 --channels=1

  # With voice, temperature, seed and word timing JSON:
  SPEAK_TIMING=output.json PTTS_VOICE=voice1 PTTS_TEMPERATURE=0.3 PTTS_SEED=42 \
    termux-clipboard-get | {prog} | pacat --raw --format=s16le --rate=24000 --channels=1

  # Custom model & dict paths + save to WAV:
  PTTS_MODEL=/path/to/model DICT_DIR=/path/to/dict \
    {prog} "تست مدل سفارشی" | ffmpeg -y -f s16le -ar 24000 -ac 1 -i - out.wav"#,
        prog = prog,
        temp = DEFAULT_TEMPERATURE,
        seed = DEFAULT_SEED
    );
}

fn env_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn model_base_dir(model_path: &str) -> PathBuf {
    let p = Path::new(model_path);
    if p.is_dir() || !p.exists() {
        p.to_path_buf()
    } else {
        p.parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

fn resolve_dict_file(env_key: &str, filename: &str) -> Option<String> {
    if let Some(v) = env_non_empty(env_key) {
        return Some(v);
    }
    if let Some(dir) = env_non_empty("DICT_DIR") {
        return Some(Path::new(&dir).join(filename).to_string_lossy().into_owned());
    }
    let local_dict = Path::new(DEFAULT_DICT_DIR).join(filename);
    if local_dict.exists() {
        return Some(local_dict.to_string_lossy().into_owned());
    }
    if let Some(dir) = exe_dir() {
        let exe_dict = dir.join("dict").join(filename);
        if exe_dict.exists() {
            return Some(exe_dict.to_string_lossy().into_owned());
        }
    }
    let cwd_file = Path::new(filename);
    if cwd_file.exists() {
        return Some(cwd_file.to_string_lossy().into_owned());
    }
    None
}

fn last_error_str(h: *mut PttsHandle) -> String {
    unsafe {
        let p = ptts_last_error(h);
        if p.is_null() {
            "unknown error".to_string()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

extern "C" fn on_audio(pcm: *const f32, n: usize, user: *mut c_void) -> bool {
    if pcm.is_null() || user.is_null() {
        return false;
    }
    unsafe {
        let buf = &mut *(user as *mut Vec<f32>);
        let samples = slice::from_raw_parts(pcm, n);
        buf.extend_from_slice(samples);
    }
    true
}

// ---------------------------------------------------------------------
// PCM output: signed 16-bit little-endian
// ---------------------------------------------------------------------

fn write_pcm(stdout: &mut impl Write, pcm: &[f32]) -> bool {
    let mut bytes = [0u8; 2048 * 2];
    for chunk in pcm.chunks(2048) {
        let mut idx = 0;
        for &s0 in chunk {
            let s = s0.clamp(-1.0, 1.0);
            let v = (s * 32767.0) as i16;
            let le = v.to_le_bytes();
            bytes[idx] = le[0];
            bytes[idx + 1] = le[1];
            idx += 2;
        }
        if stdout.write_all(&bytes[..idx]).is_err() {
            return false;
        }
    }
    true
}

fn write_silence(stdout: &mut impl Write, milliseconds: usize) -> bool {
    const SILENCE: [u8; 2048 * 2] = [0u8; 2048 * 2];
    let mut remaining = (SAMPLE_RATE * milliseconds) / 1000;
    while remaining > 0 {
        let count = remaining.min(2048);
        if stdout.write_all(&SILENCE[..count * 2]).is_err() {
            return false;
        }
        remaining -= count;
    }
    true
}

// ---------------------------------------------------------------------
// Trim silence (anchor + backtrack)
// ---------------------------------------------------------------------

fn trim_silence(audio: &mut Vec<f32>) {
    let n = audio.len();
    if n == 0 {
        return;
    }

    let sum: f64 = audio.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let global_rms = (sum / n as f64).sqrt();
    if global_rms <= 0.0 {
        return;
    }

    let window_size = (SAMPLE_RATE * TRIM_WINDOW_MS) / 1000;
    let keep_samples = (SAMPLE_RATE * TRIM_KEEP_MS) / 1000;

    let threshold = global_rms * TRIM_THRESHOLD;
    let anchor_threshold = global_rms * 0.30;
    let backtrack_threshold = global_rms * 0.15;

    let mut first_active = usize::MAX;
    let mut anchor_start = usize::MAX;
    let mut last_active = 0usize;

    let mut start = 0;
    while start < n {
        let end = (start + window_size).min(n);
        let count = end - start;
        if count > 0 {
            let wsum: f64 = audio[start..end]
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum();
            let rms = (wsum / count as f64).sqrt();

            if rms > threshold {
                if first_active == usize::MAX {
                    first_active = start;
                }
                last_active = end;
            }
            if anchor_start == usize::MAX && rms > anchor_threshold {
                anchor_start = start;
            }
        }
        start += window_size;
    }

    if first_active == usize::MAX {
        return;
    }

    if anchor_start != usize::MAX {
        first_active = anchor_start;
        while first_active >= window_size {
            let prev_start = first_active - window_size;
            let wsum: f64 = audio[prev_start..first_active]
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum();
            if (wsum / window_size as f64).sqrt() <= backtrack_threshold {
                break;
            }
            first_active = prev_start;
        }
    }

    let trim_start = first_active.saturating_sub(keep_samples);
    let keep_end = (SAMPLE_RATE * 100) / 1000;
    let trim_end = (last_active + keep_end).min(n);

    if trim_start == 0 && trim_end == n {
        return;
    }

    audio.copy_within(trim_start..trim_end, 0);
    audio.truncate(trim_end - trim_start);
}

// ---------------------------------------------------------------------
// Mute start and fade in/out
// ---------------------------------------------------------------------

fn clean_start(audio: &mut [f32]) {
    let n = audio.len();
    if n == 0 {
        return;
    }

    let mute_samples = (SAMPLE_RATE * CLEAN_MUTE_MS) / 1000;
    let fade_samples = (SAMPLE_RATE * CLEAN_FADE_MS) / 1000;

    let mute_end = mute_samples.min(n);
    for s in &mut audio[..mute_end] {
        *s = 0.0;
    }

    let fade_end = (mute_samples + fade_samples).min(n);
    let fade_length = fade_end.saturating_sub(mute_samples);
    for i in 0..fade_length {
        let gain = if fade_length > 1 {
            i as f32 / (fade_length - 1) as f32
        } else {
            0.0
        };
        audio[mute_samples + i] *= gain;
    }

    let out_len = fade_samples.min(n);
    let out_start = n - out_len;
    for i in 0..out_len {
        let gain = if out_len > 1 {
            (out_len - 1 - i) as f32 / (out_len - 1) as f32
        } else {
            0.0
        };
        audio[out_start + i] *= gain;
    }
}

// ---------------------------------------------------------------------
// Exact PERSIAN_LETTERS
// ---------------------------------------------------------------------

fn is_persian_letter(c: char) -> bool {
    matches!(
        c as u32,
        0x0622 | 0x0627 | 0x0628 | 0x067E | 0x062A | 0x062B | 0x062C | 0x0686
            | 0x062D | 0x062E | 0x062F | 0x0630 | 0x0631 | 0x0632 | 0x0698 | 0x0633
            | 0x0634 | 0x0635 | 0x0636 | 0x0637 | 0x0638 | 0x0639 | 0x063A | 0x0641
            | 0x0642 | 0x06A9 | 0x06AF | 0x0644 | 0x0645 | 0x0646 | 0x0648 | 0x0647
            | 0x06CC | 0x0626
    )
}

fn count_persian_letters(word: &str) -> usize {
    word.chars().filter(|&c| is_persian_letter(c)).count()
}

fn count_vowels(word: &str) -> usize {
    word.chars()
        .filter(|&c| matches!(c as u32, 0x0627 | 0x0622 | 0x0648 | 0x06CC | 0x0626))
        .count()
}

fn count_zwnj(word: &str) -> usize {
    word.chars().filter(|&c| c == '\u{200C}').count()
}

fn last_codepoint(word: &str) -> u32 {
    word.chars().last().map(|c| c as u32).unwrap_or(0)
}

// ---------------------------------------------------------------------
// Timing model
// ---------------------------------------------------------------------

#[derive(Default)]
struct TimingModel {
    weights: [f64; 9],
    bias: HashMap<String, f64>,
    loaded: bool,
}

impl TimingModel {
    fn load(path: &str) -> Option<Self> {
        let Ok(json) = fs::read_to_string(path) else {
            eprintln!("cannot open timing model: {}", path);
            return None;
        };
        let Ok(root) = serde_json::from_str::<Value>(&json) else {
            eprintln!("invalid timing model JSON");
            return None;
        };

        let Some(weights_arr) = root.get("weights").and_then(|v| v.as_array()) else {
            eprintln!("invalid timing model structure");
            return None;
        };
        let Some(bias_obj) = root.get("bias").and_then(|v| v.as_object()) else {
            eprintln!("invalid timing model structure");
            return None;
        };
        if weights_arr.len() != 9 {
            eprintln!("invalid timing model structure");
            return None;
        }

        let mut weights = [0.0f64; 9];
        for (i, item) in weights_arr.iter().enumerate() {
            let Some(w) = item.as_f64() else {
                eprintln!("invalid timing model weights");
                return None;
            };
            weights[i] = w;
        }

        let mut bias = HashMap::with_capacity(bias_obj.len());
        for (k, v) in bias_obj {
            if let Some(val) = v.as_f64() {
                bias.insert(k.clone(), val);
            }
        }

        Some(Self {
            weights,
            bias,
            loaded: true,
        })
    }
}

fn word_without_trailing_punctuation(word: &str) -> &str {
    word.trim_end_matches(|c: char| {
        matches!(c as u32, 0x060C | 0x061B | 0x061F) || matches!(c, ':' | '.' | '!')
    })
}

fn expected_duration(word: &str, index: usize, n: usize, model: &TimingModel) -> f64 {
    let l = count_persian_letters(word) as f64;
    let v = count_vowels(word) as f64;
    let z = count_zwnj(word) as f64;
    let last = last_codepoint(word);

    let f = [
        1.0,
        l,
        v,
        z,
        if last == 0x060C || last == 0x061B || last == ':' as u32 { 1.0 } else { 0.0 },
        if last == '.' as u32 || last == 0x061F || last == '!' as u32 { 1.0 } else { 0.0 },
        if index == 0 { 1.0 } else { 0.0 },
        if index + 2 == n { 1.0 } else { 0.0 },
        l.sqrt(),
    ];

    let mut duration: f64 = f.iter().zip(model.weights.iter()).map(|(a, b)| a * b).sum();
    let key = word_without_trailing_punctuation(word);
    if let Some(&b) = model.bias.get(key) {
        duration += b;
    }

    duration.max(0.05)
}

// ---------------------------------------------------------------------
// Word timings
// ---------------------------------------------------------------------

struct WordTimings {
    words: Vec<String>,
    starts: Vec<f64>,
}

fn compute_word_timings(text: &str, audio: &[f32], model: &TimingModel) -> WordTimings {
    let words: Vec<String> = text
        .split(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();

    let count = words.len();
    if count == 0 {
        return WordTimings {
            words,
            starts: Vec::new(),
        };
    }

    let mut starts = vec![0.0f64; count];
    let win = ((SAMPLE_RATE * TIMING_WINDOW_MS) / 1000).max(1);
    let hop = ((SAMPLE_RATE * TIMING_HOP_MS) / 1000).max(1);

    let env_count = if audio.len() >= win {
        1 + (audio.len() - win) / hop
    } else {
        0
    };
    let duration = audio.len() as f64 / SAMPLE_RATE as f64;

    if env_count < 4 {
        for (i, st) in starts.iter_mut().enumerate() {
            *st = duration * (i as f64) / (count as f64);
        }
        return WordTimings { words, starts };
    }

    let mut env = vec![0.0f64; env_count];
    for i in 0..env_count {
        let start = i * hop;
        let sum: f64 = audio[start..start + win]
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum();
        env[i] = (sum / win as f64).sqrt();
    }

    let total_sum: f64 = audio.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let threshold = 0.04 * (total_sum / audio.len() as f64).sqrt();

    let mut first = usize::MAX;
    let mut last = 0usize;
    for (i, &e) in env.iter().enumerate() {
        if e > threshold {
            if first == usize::MAX {
                first = i;
            }
            last = i;
        }
    }
    if first == usize::MAX {
        first = 0;
        last = env_count - 1;
    }

    let t0 = (first * hop) as f64 / SAMPLE_RATE as f64;
    let t1 = (((last * hop + win) as f64) / SAMPLE_RATE as f64).min(duration);

    let mut sm = vec![0.0f64; env_count];
    for i in 0..env_count {
        let a = i.saturating_sub(1);
        let b = (i + 1).min(env_count - 1);
        sm[i] = (env[a] + env[i] + env[b]) / 3.0;
    }

    let mut share_start = vec![0.0f64; count];
    let mut word_end = vec![0.0f64; count];
    let mut pauses = vec![0.0f64; count];
    let mut pos = 0.0f64;

    for i in 0..count {
        share_start[i] = pos;
        if model.loaded {
            pos += expected_duration(&words[i], i, count + 1, model);
        } else {
            pos += count_persian_letters(&words[i]) as f64 + 1.5;
        }

        let last_char = last_codepoint(&words[i]);
        if !model.loaded && i < count - 1 {
            if last_char == 0x060C || last_char == ':' as u32 {
                pauses[i] = 4.0;
            } else if last_char == 0x061B {
                pauses[i] = 5.0;
            }
        }

        word_end[i] = pos;
        pos += pauses[i];
    }

    if pos <= 0.0 {
        pos = 1.0;
    }
    let scale = (t1 - t0) / pos;

    for i in 0..count {
        starts[i] = t0 + share_start[i] * scale;
    }

    let mut previous = t0;
    for i in 1..count {
        let gap_start = t0 + word_end[i - 1] * scale;
        let gap_end = starts[i];

        let radius = ((TIMING_SEARCH_MS / TIMING_HOP_MS) as isize).max(1);
        let mut lo = ((gap_start * SAMPLE_RATE as f64) / hop as f64).round() as isize - radius;
        let mut hi = ((gap_end * SAMPLE_RATE as f64) / hop as f64).round() as isize + radius;

        if lo < 1 {
            lo = 1;
        }
        if hi > env_count as isize - 2 {
            hi = env_count as isize - 2;
        }
        if lo > hi {
            lo = hi;
        }
        if lo < 1 {
            lo = 1;
        }

        let mut minimum = lo;
        for k in lo..=hi {
            if sm[k as usize] < sm[minimum as usize] {
                minimum = k;
            }
        }

        let mut k = minimum;
        if pauses[i - 1] > 0.0 {
            while k < env_count as isize - 2 && sm[k as usize] < 2.0 * threshold && k < hi {
                k += 1;
            }
        }

        let mut boundary = (k as f64 * hop as f64) / SAMPLE_RATE as f64;
        if boundary < previous + TIMING_MIN_GAP {
            boundary = previous + TIMING_MIN_GAP;
        }
        let max_boundary = t1 - TIMING_MIN_GAP * (count - i) as f64;
        if boundary > max_boundary {
            boundary = max_boundary;
        }

        starts[i] = boundary;
        previous = boundary;
    }

    starts[0] = (t0 - 0.03).max(0.0);
    WordTimings { words, starts }
}

// ---------------------------------------------------------------------
// JSON string escaping & writing
// ---------------------------------------------------------------------

fn json_write_string(fp: &mut impl Write, s: &str) -> io::Result<()> {
    fp.write_all(b"\"")?;
    for &c in s.as_bytes() {
        match c {
            b'"' => fp.write_all(b"\\\"")?,
            b'\\' => fp.write_all(b"\\\\")?,
            0x08 => fp.write_all(b"\\b")?,
            0x0C => fp.write_all(b"\\f")?,
            b'\n' => fp.write_all(b"\\n")?,
            b'\r' => fp.write_all(b"\\r")?,
            b'\t' => fp.write_all(b"\\t")?,
            0x00..=0x1F => write!(fp, "\\u{:04x}", c)?,
            _ => fp.write_all(&[c])?,
        }
    }
    fp.write_all(b"\"")
}

fn write_json_timings(
    fp: &mut impl Write,
    first_word: &mut bool,
    timings: &WordTimings,
    offset: f64,
    piece_duration: f64,
) -> bool {
    let count = timings.words.len();
    for i in 0..count {
        let mut start = timings.starts[i].clamp(0.0, piece_duration);
        let mut end = if i + 1 < count {
            timings.starts[i + 1]
        } else {
            piece_duration
        };
        if end > piece_duration {
            end = piece_duration;
        }
        if end <= start {
            end = (start + 0.05).min(piece_duration);
            if end <= start {
                start = if end > 0.0 { end - 0.001 } else { 0.0 };
            }
        }

        if !*first_word && fp.write_all(b",\n").is_err() {
            return false;
        }
        *first_word = false;

        if fp.write_all(b"    {\"word\":").is_err() {
            return false;
        }
        if json_write_string(fp, &timings.words[i]).is_err() {
            return false;
        }
        if write!(fp, ",\"start\":{:.6},\"end\":{:.6}}}", offset + start, offset + end).is_err() {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------
// Speak one piece & full text
// ---------------------------------------------------------------------

fn speak_sentence(
    h: *mut PttsHandle,
    stdout: &mut impl Write,
    sentence: &str,
    add_pause: bool,
    timing_file: Option<&mut BufWriter<File>>,
    first_word: &mut bool,
    global_offset: &mut f64,
    timing_model: &TimingModel,
) -> bool {
    if sentence.is_empty() {
        return true;
    }

    let Ok(c_text) = CString::new(sentence) else {
        eprintln!("cannot allocate sentence");
        return false;
    };

    let mut audio: Vec<f32> = Vec::with_capacity(16384);
    let mut result = PttsResult { _opaque: [0; 16] };

    let mut ok = unsafe {
        ptts_speak(
            h,
            c_text.as_ptr(),
            on_audio,
            (&mut audio as *mut Vec<f32>) as *mut c_void,
            &mut result,
        )
    };

    if !ok {
        eprintln!("cannot speak: {}", last_error_str(h));
    } else {
        trim_silence(&mut audio);
        clean_start(&mut audio);

        let piece_duration = audio.len() as f64 / SAMPLE_RATE as f64;

        if let Some(tf) = timing_file {
            if !audio.is_empty() {
                let timings = compute_word_timings(sentence, &audio, timing_model);
                ok = write_json_timings(tf, first_word, &timings, *global_offset, piece_duration);
            }
        }

        if ok && !audio.is_empty() {
            ok = write_pcm(stdout, &audio);
        }

        if ok {
            *global_offset += piece_duration;
        }
    }

    if ok && add_pause {
        ok = write_silence(stdout, PAUSE_MS);
        if ok {
            *global_offset += PAUSE_MS as f64 / 1000.0;
        }
    }

    ok
}

fn speak_text(
    h: *mut PttsHandle,
    stdout: &mut impl Write,
    model_dir: &Path,
    text: &str,
    mut timing_file: Option<&mut BufWriter<File>>,
    first_word: &mut bool,
    global_offset: &mut f64,
    timing_model: &TimingModel,
) -> bool {
    let tokenizer_json = env_non_empty("TOKENIZER_JSON").unwrap_or_else(|| {
        model_dir
            .join("tokenizer.json")
            .to_string_lossy()
            .into_owned()
    });

    let cmu_path = resolve_dict_file("G2P_CMUDICT", "cmudict.txt");
    let latin_path = resolve_dict_file("G2P_LATIN_DICT", "latin_dict.txt");
    let normalized_text = normalize_with_dicts(
        text,
        cmu_path.as_deref(),
        latin_path.as_deref(),
    );
    let normalized_text = normalized_text.trim();
    if normalized_text.is_empty() {
        return true;
    }

    let mut tokenizer = Tokenizer::new();
    if !tokenizer.load(&tokenizer_json) {
        eprintln!("tokenizer failed (using {})", tokenizer_json);
        return false;
    }

    let pieces = tokenizer.split_sentences(normalized_text, 80, 8);

    for (idx, piece) in pieces.iter().enumerate() {
        let has_next = idx + 1 < pieces.len();
        let ok = speak_sentence(
            h,
            stdout,
            piece,
            has_next,
            timing_file.as_deref_mut(),
            first_word,
            global_offset,
            timing_model,
        );
        if !ok {
            return false;
        }
    }

    true
}

// ---------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().collect();
    let prog = &args[0];

    if args.len() == 2 && (args[1] == "-h" || args[1] == "--help") {
        print_help(prog);
        std::process::exit(0);
    }

    let text = match args.len() {
        1 => {
            if io::stdin().is_terminal() {
                eprintln!(
                    "usage:\n  {} TEXT\n  echo \"TEXT\" | {}\n\nRun '{} --help' for options and environment variables.",
                    prog, prog, prog
                );
                std::process::exit(2);
            }
            let mut buf = String::new();
            if io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("failed to read stdin");
                std::process::exit(1);
            }
            buf
        }
        2 if args[1] == "-" => {
            let mut buf = String::new();
            if io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("failed to read stdin");
                std::process::exit(1);
            }
            buf
        }
        2 => args[1].clone(),
        _ => {
            eprintln!(
                "usage:\n  {} TEXT\n  echo \"TEXT\" | {}\n\nRun '{} --help' for options and environment variables.",
                prog, prog, prog
            );
            std::process::exit(2);
        }
    };

    if text.trim().is_empty() {
        std::process::exit(0);
    }

    let model = env_non_empty("PTTS_MODEL").unwrap_or_else(|| DEFAULT_MODEL_DIR.to_string());
    let lang = env_non_empty("PTTS_LANG").unwrap_or_else(|| DEFAULT_LANG.to_string());
    let voice_arg = Some(env_non_empty("PTTS_VOICE").unwrap_or_else(|| DEFAULT_VOICE.to_string()));
    let temp_arg = env_non_empty("PTTS_TEMPERATURE");
    let seed_arg = env_non_empty("PTTS_SEED");

    let model_dir = model_base_dir(&model);

    let c_model = CString::new(model.as_str()).expect("invalid model path");
    let c_lang = CString::new(lang.as_str()).expect("invalid lang");

    let h = unsafe { ptts_new(c_model.as_ptr(), PTTS_UNIT_DEFAULT, c_lang.as_ptr()) };
    if h.is_null() {
        eprintln!(
            "cannot load {}: {}",
            model,
            last_error_str(std::ptr::null_mut())
        );
        std::process::exit(1);
    }

    if let Some(voice) = voice_arg {
        let c_voice = CString::new(voice.as_str()).unwrap_or_default();
        let ok = unsafe { ptts_set_voice(h, c_voice.as_ptr()) };
        if !ok {
            eprintln!("{}. The voices are:", last_error_str(h));
            unsafe {
                let mut p = ptts_voices(h);
                while !p.is_null() && *p != 0 {
                    let s = CStr::from_ptr(p);
                    eprintln!("  {}", s.to_string_lossy());
                    p = p.add(s.to_bytes().len() + 1);
                }
                ptts_free(h);
            }
            std::process::exit(1);
        }
    }

    let temperature = if let Some(t_str) = temp_arg {
        match t_str.parse::<f32>() {
            Ok(t) if t.is_finite() => t,
            _ => {
                eprintln!("bad temperature {}", t_str);
                unsafe { ptts_free(h) };
                std::process::exit(1);
            }
        }
    } else {
        DEFAULT_TEMPERATURE
    };
    if !unsafe { ptts_set_temperature(h, temperature) } {
        eprintln!("cannot set temperature {}: {}", temperature, last_error_str(h));
        unsafe { ptts_free(h) };
        std::process::exit(1);
    }

    let seed = if let Some(s_str) = seed_arg {
        if s_str.starts_with('-') {
            eprintln!("bad seed {}", s_str);
            unsafe { ptts_free(h) };
            std::process::exit(1);
        }
        match s_str.parse::<u64>() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("bad seed {}", s_str);
                unsafe { ptts_free(h) };
                std::process::exit(1);
            }
        }
    } else {
        DEFAULT_SEED
    };
    unsafe { ptts_set_seed(h, seed) };

    // Optional JSON timing output
    let mut timing_file: Option<BufWriter<File>> = None;
    let mut timing_model = TimingModel::default();
    let mut first_word = true;
    let mut global_offset = 0.0f64;

    if let Some(timing_path) = env_non_empty("SPEAK_TIMING") {
        let timing_model_path = env_non_empty("SPEAK_TIMING_MODEL").unwrap_or_else(|| {
            model_dir
                .join("timing_model.json")
                .to_string_lossy()
                .into_owned()
        });

        let Some(m) = TimingModel::load(&timing_model_path) else {
            unsafe { ptts_free(h) };
            std::process::exit(1);
        };
        timing_model = m;

        let Ok(f) = File::create(&timing_path) else {
            eprintln!("cannot create timing JSON: {}", timing_path);
            unsafe { ptts_free(h) };
            std::process::exit(1);
        };
        let mut w = BufWriter::new(f);
        if write!(w, "{{\n  \"sample_rate\": {},\n  \"words\": [\n", SAMPLE_RATE).is_err() {
            unsafe { ptts_free(h) };
            std::process::exit(1);
        }
        timing_file = Some(w);
    }

    let stdout_raw = io::stdout();
    let mut stdout = BufWriter::new(stdout_raw.lock());

    let mut ok = speak_text(
        h,
        &mut stdout,
        &model_dir,
        &text,
        timing_file.as_mut(),
        &mut first_word,
        &mut global_offset,
        &timing_model,
    );

    if stdout.flush().is_err() {
        ok = false;
    }

    if let Some(mut tf) = timing_file {
        if tf.write_all(b"\n  ]\n}\n").is_err() || tf.flush().is_err() {
            ok = false;
        }
    }

    unsafe { ptts_free(h) };
    std::process::exit(if ok { 0 } else { 1 });
}

