// SentencePiece Precompiled charsmap + BPE tokenizer (count, encode token IDs, sentence splitter).

use serde_json::Value;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::ptr;

fn le32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

fn utf8_len(c: u8) -> usize {
    if (c & 0xE0) == 0xC0 {
        2
    } else if (c & 0xF0) == 0xE0 {
        3
    } else if (c & 0xF8) == 0xF0 {
        4
    } else {
        1
    }
}

fn base64_decode(s: &str) -> Vec<u8> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut val: i32 = 0;
    let mut bits: i32 = -8;

    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(p) = CHARS.iter().position(|&x| x == c) else {
            continue;
        };
        val = (val << 6) | (p as i32);
        bits += 6;
        if bits >= 0 {
            out.push(((val >> bits) & 255) as u8);
            bits -= 8;
        }
    }

    out
}

fn utf8_chars(s: &str) -> Vec<String> {
    s.chars().map(|c| c.to_string()).collect()
}

#[derive(Default)]
struct Precompiled {
    trie: Vec<u32>,
    normalized: Vec<u8>,
}

impl Precompiled {
    fn load(&mut self, encoded: &str) -> bool {
        let data = base64_decode(encoded);
        if data.len() < 4 {
            return false;
        }

        let trie_size = le32(&data[0..4]) as usize;
        if trie_size % 4 != 0 || trie_size + 4 > data.len() {
            return false;
        }

        let count = trie_size / 4;
        if count == 0 {
            return false;
        }

        self.trie.clear();
        self.trie.reserve(count);
        for i in 0..count {
            let off = 4 + i * 4;
            self.trie.push(le32(&data[off..off + 4]));
        }

        self.normalized = data[4 + trie_size..].to_vec();
        true
    }

    fn has_leaf(u: u32) -> bool {
        ((u >> 8) & 1) != 0
    }

    fn label(u: u32) -> u32 {
        u & ((1u32 << 31) | 0xFF)
    }

    fn offset(u: u32) -> usize {
        ((u >> 10) as usize) << (((u & (1u32 << 9)) >> 6) as usize)
    }

    fn longest_match(&self, s: &[u8], pos: usize) -> Option<(usize, usize)> {
        if self.trie.is_empty() {
            return None;
        }

        let mut node: usize = 0;
        let mut unit = self.trie[node];
        node ^= Self::offset(unit);

        let mut found: Option<(usize, usize)> = None;

        for i in pos..s.len() {
            let c = s[i];
            if c == 0 {
                break;
            }
            node ^= c as usize;
            if node >= self.trie.len() {
                break;
            }
            unit = self.trie[node];
            if Self::label(unit) != c as u32 {
                break;
            }
            node ^= Self::offset(unit);
            if node >= self.trie.len() {
                break;
            }
            if Self::has_leaf(unit) {
                let value = (self.trie[node] & 0x7FFF_FFFF) as usize;
                let len = i - pos + 1;
                found = Some((len, value));
            }
        }

        found
    }

    fn normalize(&self, input: &str) -> String {
        let bytes = input.as_bytes();
        let mut output: Vec<u8> = Vec::with_capacity(bytes.len() + 16);
        let mut i = 0;

        while i < bytes.len() {
            if let Some((len, value)) = self.longest_match(bytes, i) {
                if value < self.normalized.len() {
                    if let Some(rel_end) = self.normalized[value..].iter().position(|&b| b == 0) {
                        output.extend_from_slice(&self.normalized[value..value + rel_end]);
                        i += len;
                        continue;
                    }
                }
            }

            let mut n = utf8_len(bytes[i]);
            if i + n > bytes.len() {
                n = 1;
            }
            output.extend_from_slice(&bytes[i..i + n]);
            i += n;
        }

        String::from_utf8(output)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

pub struct Tokenizer {
    vocab: HashMap<String, i32>,
    ranks: HashMap<String, i32>,
    precompiled: Precompiled,
    prepend: String,
    strip_left: bool,
    strip_right: bool,
    fuse_unk: bool,
    unk_id: i32,
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        Self {
            vocab: HashMap::new(),
            ranks: HashMap::new(),
            precompiled: Precompiled::default(),
            prepend: String::new(),
            strip_left: true,
            strip_right: true,
            fuse_unk: true,
            unk_id: 0,
        }
    }

    pub fn load(&mut self, path: &str) -> bool {
        let Ok(json) = fs::read_to_string(path) else {
            eprintln!("Cannot open tokenizer.json");
            return false;
        };

        let Ok(root) = serde_json::from_str::<Value>(&json) else {
            eprintln!("Invalid tokenizer.json");
            return false;
        };
        drop(json);

        let Some(model) = root.get("model") else {
            eprintln!("Invalid BPE model");
            return false;
        };
        let Some(v) = model.get("vocab").and_then(|x| x.as_object()) else {
            eprintln!("Invalid BPE model");
            return false;
        };
        let Some(m) = model.get("merges").and_then(|x| x.as_array()) else {
            eprintln!("Invalid BPE model");
            return false;
        };

        self.vocab.clear();
        self.vocab.reserve(v.len() * 2);
        for (k, val) in v {
            if let Some(id) = val.as_i64() {
                self.vocab.insert(k.clone(), id as i32);
            }
        }

        self.unk_id = model
            .get("unk_token")
            .and_then(|x| x.as_str())
            .and_then(|tok| self.vocab.get(tok).copied())
            .or_else(|| self.vocab.get("<unk>").copied())
            .unwrap_or(0);

        self.ranks.clear();
        self.ranks.reserve(m.len() * 2);
        for (rank, pair) in m.iter().enumerate() {
            let Some(arr) = pair.as_array() else {
                continue;
            };
            if arr.len() != 2 {
                continue;
            }
            let (Some(a), Some(b)) = (arr[0].as_str(), arr[1].as_str()) else {
                continue;
            };
            let mut key = String::with_capacity(a.len() + b.len() + 1);
            key.push_str(a);
            key.push('\x1f');
            key.push_str(b);
            self.ranks.insert(key, rank as i32);
        }

        if let Some(list) = root
            .get("normalizer")
            .and_then(|n| n.get("normalizers"))
            .and_then(|l| l.as_array())
        {
            for n in list {
                let Some(t) = n.get("type").and_then(|x| x.as_str()) else {
                    continue;
                };
                match t {
                    "Precompiled" => {
                        let ok = n
                            .get("precompiled_charsmap")
                            .and_then(|x| x.as_str())
                            .map(|s| self.precompiled.load(s))
                            .unwrap_or(false);
                        if !ok {
                            eprintln!("Cannot load Precompiled charsmap");
                            return false;
                        }
                    }
                    "Strip" => {
                        self.strip_left = n.get("left").and_then(|x| x.as_bool()) != Some(false);
                        self.strip_right = n.get("right").and_then(|x| x.as_bool()) != Some(false);
                    }
                    "Prepend" => {
                        if let Some(content) = n.get("prepend").and_then(|x| x.as_str()) {
                            self.prepend = content.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(fu) = model.get("fuse_unk").and_then(|x| x.as_bool()) {
            self.fuse_unk = fu;
        }

        true
    }

    fn normalize_text(&self, input: &str) -> String {
        let s = self.precompiled.normalize(input);
        let mut s_slice = s.as_str();

        if self.strip_left {
            s_slice = s_slice.trim_start_matches(' ');
        }
        if self.strip_right {
            s_slice = s_slice.trim_end_matches(' ');
        }

        let mut collapsed = String::with_capacity(s_slice.len());
        let bytes = s_slice.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b' ' {
                collapsed.push(' ');
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
            } else {
                let ch = s_slice[i..].chars().next().unwrap();
                collapsed.push(ch);
                i += ch.len_utf8();
            }
        }

        if collapsed.is_empty() {
            return String::new();
        }

        let full = format!("{}{}", self.prepend, collapsed);
        let mut result = String::with_capacity(full.len() + 16);
        for c in full.chars() {
            if c == ' ' {
                result.push('\u{2581}');
            } else {
                result.push(c);
            }
        }
        result
    }

    fn bpe_pieces(&self, input: &str) -> Vec<String> {
        let mut pieces = utf8_chars(&self.normalize_text(input));
        let mut key = String::with_capacity(64);
        let mut merged = String::with_capacity(64);

        while pieces.len() > 1 {
            let mut best_rank = i32::MAX;
            let mut best_pos = pieces.len();

            for i in 0..pieces.len() - 1 {
                key.clear();
                key.push_str(&pieces[i]);
                key.push('\x1f');
                key.push_str(&pieces[i + 1]);

                let Some(&rank) = self.ranks.get(&key) else {
                    continue;
                };
                if rank >= best_rank {
                    continue;
                }

                merged.clear();
                merged.push_str(&pieces[i]);
                merged.push_str(&pieces[i + 1]);
                if !self.vocab.contains_key(&merged) {
                    continue;
                }

                best_rank = rank;
                best_pos = i;
            }

            if best_pos == pieces.len() {
                break;
            }

            let next = pieces.remove(best_pos + 1);
            pieces[best_pos].push_str(&next);
        }

        pieces
    }

    pub fn encode(&self, input: &str) -> Vec<i32> {
        let pieces = self.bpe_pieces(input);
        let mut ids = Vec::with_capacity(pieces.len());
        let mut previous_unknown = false;

        for piece in &pieces {
            if let Some(&id) = self.vocab.get(piece) {
                ids.push(id);
                previous_unknown = false;
            } else {
                if !self.fuse_unk || !previous_unknown {
                    ids.push(self.unk_id);
                }
                previous_unknown = true;
            }
        }

        ids
    }

    pub fn count(&self, input: &str) -> i32 {
        self.encode(input).len() as i32
    }

    fn trim_ws(s: &str) -> &str {
        s.trim_matches(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n')
    }

    fn split_by_spaces(s: &str) -> Vec<&str> {
        s.split(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n')
            .filter(|w| !w.is_empty())
            .collect()
    }

    fn split_oversized(&self, text: &str, max_tokens: i32) -> Vec<String> {
        if self.count(text) <= max_tokens {
            return vec![text.to_string()];
        }

        let mut parts: Vec<String> = Vec::new();
        let mut chunk = String::new();

        let flush = |parts: &mut Vec<String>, chunk: &mut String| {
            let s = Self::trim_ws(chunk);
            if !s.is_empty() {
                parts.push(s.to_string());
            }
            chunk.clear();
        };

        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let mut n = utf8_len(bytes[i]);
            if i + n > bytes.len() {
                n = 1;
            }

            let soft = (n == 2 && bytes[i] == 0xD8 && (bytes[i + 1] == 0x8C || bytes[i + 1] == 0x9B))
                || (n == 1 && bytes[i] == b':');

            chunk.push_str(&text[i..i + n]);
            i += n;

            if soft && i < bytes.len() && bytes[i] == b' ' {
                let candidate = Self::trim_ws(&chunk);
                if !candidate.is_empty() && self.count(candidate) >= 8 {
                    flush(&mut parts, &mut chunk);
                }
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
            }
        }
        flush(&mut parts, &mut chunk);

        let mut result: Vec<String> = Vec::new();
        let word_limit = 1.max(max_tokens / 2);

        for part in parts {
            if self.count(&part) <= max_tokens {
                result.push(part);
                continue;
            }

            let words = Self::split_by_spaces(&part);
            let mut line = String::new();

            for word in words {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{} {}", line, word)
                };

                if !line.is_empty() && self.count(&candidate) > word_limit {
                    result.push(line);
                    line = word.to_string();
                } else {
                    line = candidate;
                }
            }

            if !line.is_empty() {
                result.push(line);
            }
        }

        result
    }

    pub fn split_sentences(&self, input: &str, max_tokens: i32, min_tokens: i32) -> Vec<String> {
        let mut text = Self::trim_ws(input).to_string();
        if text.is_empty() {
            return Vec::new();
        }

        if !text.ends_with('.')
            && !text.ends_with('!')
            && !text.ends_with('?')
            && !text.ends_with('؟')
        {
            text.push('.');
        }

        let bytes = text.as_bytes();
        let mut sentences: Vec<String> = Vec::new();
        let mut start = 0;
        let mut i = 0;

        while i < bytes.len() {
            let mut end = i + 1;
            let mut punctuation = false;

            if matches!(bytes[i], b'.' | b'!' | b'?') {
                punctuation = true;
            } else if i + 1 < bytes.len() && bytes[i] == 0xD8 && bytes[i + 1] == 0x9F {
                punctuation = true;
                end = i + 2;
            }

            if punctuation
                && (end == bytes.len() || matches!(bytes[end], b' ' | b'\t' | b'\n'))
            {
                let s = Self::trim_ws(&text[start..end]);
                if !s.is_empty() {
                    sentences.push(s.to_string());
                }
                start = end;
                while start < bytes.len() && matches!(bytes[start], b' ' | b'\t' | b'\n') {
                    start += 1;
                }
                i = start;
            } else {
                i += 1;
            }
        }

        if start < bytes.len() {
            let s = Self::trim_ws(&text[start..]);
            if !s.is_empty() {
                sentences.push(s.to_string());
            }
        }

        let mut merged: Vec<String> = Vec::new();
        let mut last_count = 0;

        for sentence in sentences {
            if merged.is_empty() {
                last_count = self.count(&sentence);
                merged.push(sentence);
                continue;
            }

            if last_count < min_tokens {
                let candidate = format!("{} {}", merged.last().unwrap(), sentence);
                let candidate_count = self.count(&candidate);
                if candidate_count <= max_tokens {
                    *merged.last_mut().unwrap() = candidate;
                    last_count = candidate_count;
                    continue;
                }
            }

            last_count = self.count(&sentence);
            merged.push(sentence);
        }

        let mut result: Vec<String> = Vec::new();
        for sentence in merged {
            result.extend(self.split_oversized(&sentence, max_tokens));
        }
        result
    }
}

// ------------------------------------------------------------
// C / C++ / FFI API
// ------------------------------------------------------------

pub struct PttsTokenizer {
    inner: Tokenizer,
}

unsafe fn cstr_opt<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_new(
    tokenizer_json_path: *const c_char,
) -> *mut PttsTokenizer {
    let Some(path) = cstr_opt(tokenizer_json_path) else {
        return ptr::null_mut();
    };
    let mut tok = Tokenizer::new();
    if !tok.load(path) {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(PttsTokenizer { inner: tok }))
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_free(tok: *mut PttsTokenizer) {
    if !tok.is_null() {
        drop(Box::from_raw(tok));
    }
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_count(
    tok: *const PttsTokenizer,
    text: *const c_char,
) -> i32 {
    if tok.is_null() {
        return -1;
    }
    let Some(input) = cstr_opt(text) else {
        return -1;
    };
    (*tok).inner.count(input)
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_encode(
    tok: *const PttsTokenizer,
    text: *const c_char,
    out_ids: *mut i32,
    max_len: i32,
) -> i32 {
    if tok.is_null() {
        return -1;
    }
    let Some(input) = cstr_opt(text) else {
        return -1;
    };
    let ids = (*tok).inner.encode(input);
    let total = ids.len() as i32;
    if !out_ids.is_null() && max_len > 0 {
        let n = ids.len().min(max_len as usize);
        ptr::copy_nonoverlapping(ids.as_ptr(), out_ids, n);
    }
    total
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_split(
    tok: *const PttsTokenizer,
    text: *const c_char,
    max_tokens: i32,
    min_tokens: i32,
) -> *mut c_char {
    if tok.is_null() {
        return ptr::null_mut();
    }
    let Some(input) = cstr_opt(text) else {
        return ptr::null_mut();
    };
    let max_t = if max_tokens > 0 { max_tokens } else { 80 };
    let min_t = if min_tokens > 0 { min_tokens } else { 8 };
    let joined = (*tok).inner.split_sentences(input, max_t, min_t).join("\n");
    match CString::new(joined.replace('\0', "")) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ptts_tokenizer_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
