# Persian Offline TTS Bridge (`persian-tts-bridge`)

A modular, offline Persian Text-to-Speech (TTS) workspace written in Rust. It provides standalone static (`.a`) and shared (`.so`) libraries with C/C++/FFI headers for Persian text normalization and general-purpose SentencePiece BPE tokenization, alongside a single self-contained `speak` CLI binary powered by `xn-ptts` (Phonon).

## Architecture

```text
.
├── Cargo.toml                 # Cargo Workspace root
├── build.sh                   # Automated setup, quantization, and build script
├── crates/
│   ├── persian_normalizer/    # Zero-dependency Persian/mixed text normalizer (.rlib, .a, .so)
│   ├── tokenizer/             # General SentencePiece BPE tokenizer: encode(), count(), split_sentences() (.rlib, .a, .so)
│   └── speak/                 # Standalone CLI binary statically linking normalizer, tokenizer, and ptts_ffi
├── include/
│   ├── persian_normalizer.h   # C/C++/FFI header for libpersian_normalizer
│   └── tokenizer.h            # C/C++/FFI header for libtokenizer
├── dict/                      # Pronunciation dictionaries (cmudict.txt, latin_dict.txt)
├── demo/                      # Pre-generated mixed-text WAV demo recordings
└── xn-ptts/                   # Git submodule (gradium-ai/xn-ptts)
```

### Key Features

- **`persian_normalizer`**: Pure Rust, zero external crate dependencies. Normalizes Persian text, numbers, and symbols, and transliterates Latin words via `cmudict.txt` and `latin_dict.txt`.
- **`tokenizer`**: General-purpose SentencePiece Precompiled + BPE tokenizer reading standard `tokenizer.json`. Supports raw token ID encoding (`encode` / `ptts_tokenizer_encode`), token counting, and punctuation-aware sentence chunking (`split_sentences`).
- **`speak`**: Single statically-linked binary with zero subprocess overhead. Reads UTF-8 text from `stdin` or arguments and streams raw 24 kHz `s16le` mono PCM audio to `stdout`.

---

## Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- `git` (and `git-lfs` if downloading models from Hugging Face)
- Audio player for live playback (e.g., `pacat` / PulseAudio on Linux or Termux)

---

## Option 1: Automated Build (`build.sh`)

Clone the repository with submodules and run `build.sh`:

```bash
git clone --recursive https://github.com/gyroing/persian-tts-bridge.git
cd persian-tts-bridge
./build.sh
```

`build.sh` automatically checks for existing `model/` assets, optionally downloads and quantizes `mehdi-hf/cilla-mini` to `model.q8.gguf`, initializes the `xn-ptts` submodule, builds `libptts_ffi`, and compiles all workspace libraries and binaries into `target/release/`.

---

## Option 2: Manual Build (Without `build.sh`)

If you prefer to build each component step by step without running `build.sh`:

### 1. Initialize the `xn-ptts` submodule

```bash
git submodule update --init --recursive
```

### 2. Build the `ptts-ffi` audio engine

```bash
cd xn-ptts
cargo build --release -p ptts-ffi
cd ..
```

### 3. (Optional) Quantize `model.safetensors` to `model.q8.gguf`

If you have placed `model.safetensors`, `tokenizer.json`, and voice files inside `./model/`:

```bash
cd xn-ptts
cargo run --release --example quantize -- \
  ../model/model.safetensors \
  ../model/model.q8.gguf \
  --quant q8 \
  --no-mimi-encoder
cd ..
```

### 4. Build standalone `.a` and `.so` libraries

```bash
cargo build --release -p persian_normalizer -p tokenizer
```

Artifacts generated in `target/release/`:
- `libpersian_normalizer.a`, `libpersian_normalizer.so`
- `libtokenizer.a`, `libtokenizer.so`

### 5. Build the `speak` binary

```bash
PTTS_LIB_DIR="$(realpath ./xn-ptts/target/release)" \
  cargo build --release -p speak --features phonon
```

Artifact generated:
- `target/release/speak`

---

## Usage

Run from the repository root (so `./model` and `./dict` are resolved automatically):

```bash
# Direct text to PulseAudio
echo "سلام دنیا، این یک تست آفلاین است." | ./target/release/speak | \
  pacat --raw --format=s16le --rate=24000 --channels=1

# Termux clipboard pipeline
termux-clipboard-get | ./target/release/speak | \
  pacat --raw --format=s16le --rate=24000 --channels=1

# With optional synthesis parameters & word-level timing export
SPEAK_TIMING=output.json PTTS_VOICE=voice1 PTTS_TEMPERATURE=0.3 PTTS_SEED=42 \
  termux-clipboard-get | ./target/release/speak | \
  pacat --raw --format=s16le --rate=24000 --channels=1
```

---

## Demo & Mixed-Text Samples

Pre-generated sample recordings (24 kHz, 16-bit signed LE mono PCM converted to WAV) are stored under `demo/`:

1. **AI & Hardware (`demo/sample_1.wav`)**
   > در تاریخ 1404/08/25، شرکت NVIDIA از پردازنده گرافیکی جدید خود با 24.5 گیگابایت حافظه VRAM و معماری Blackwell رونمایی کرد که اجرای مدل‌های LLM در قالب GGUF را با latency زیر 18.4 میلی‌ثانیه و performance بسیار بالا روی سرورهای AWS و Azure ممکن می‌سازد.

2. **Finance, Markets & Decimals (`demo/sample_2.wav`)**
   > طبق گزارش quarterly منتشرشده در 2026/03/15، ارزش سهام Apple و Microsoft به ترتیب 3.75 و 4.2 درصد رشد داشت و سرمایه‌گذاری مشترک OpenAI و Anthropic روی زیرساخت‌های cloud و دیتاسنترهای مجهز به پردازنده‌های AMD به 12.8 میلیارد دلار رسید.

3. **Software & TTS Pipeline (`demo/sample_3.wav`)**
   > نسخه 2.4.1 کتابخانه persian-tts-bridge که با زبان Rust و Cargo نوشته شده، در تاریخ ۱۴۰۵/۰۲/۱۰ روی GitHub و HuggingFace منتشر شد و می‌تواند متن‌های فارسی و انگلیسی را با نرخ نمونه‌برداری 24000 هرتز و دقت 99.6 درصد به صورت real-time و آفلاین پردازش کند.

4. **EVs, Energy & Units (`demo/sample_4.wav`)**
   > مدل جدید Tesla در تست‌های جاده‌ای روز 1403/11/09 توانست با یک بار شارژ باتری 82.5 کیلووات‌ساعتی، مسافت 615.8 کیلومتر را با میانگین سرعت 110.5 کیلومتر بر ساعت طی کند و مصرف انرژی را نسبت به محصولات Bosch و Siemens حدود 14.2 درصد کاهش دهد.

5. **Mobile, Wireless & Streaming (`demo/sample_5.wav`)**
   > گوشی پرچم‌دار Samsung با نمایشگر OLED و تراشه Snapdragon در بنچمارک‌های تاریخ 2025/12/01 امتیاز 94.6 از 100 را کسب کرد و در آزمایش اتصال wireless و پخش زنده stream با کدک Opus روی پلتفرم YouTube تنها 1.35 درصد افت بسته‌های شبکه داشت.

### Live Playback Example

```bash
./target/release/speak "نسخه 2.4.1 کتابخانه persian-tts-bridge که با زبان Rust و Cargo نوشته شده، در تاریخ ۱۴۰۵/۰۲/۱۰ روی GitHub و HuggingFace منتشر شد و می‌تواند متن‌های فارسی و انگلیسی را با نرخ نمونه‌برداری 24000 هرتز و دقت 99.6 درصد به صورت real-time و آفلاین پردازش کند." | pacat --raw --format=s16le --rate=24000 --channels=1
```

---

## Environment Variables

### Runtime — `speak` & Synthesis Engine

| Variable | Description | Default / Example |
| :--- | :--- | :--- |
| `PTTS_VOICE` | Voice preset/speaker identifier used by the TTS engine. | Model default (e.g., `voice1`) |
| `PTTS_TEMPERATURE` | Sampling temperature controlling speech expressiveness vs. stability. | Model default (e.g., `0.3`) |
| `PTTS_SEED` | Random seed for reproducible audio generation. | Random / e.g., `42` |
| `SPEAK_TIMING` | Output file path for exporting word-level timing metadata as JSON. | Disabled (e.g., `output.json`) |
| `TOKENIZER_JSON` | Custom path to `tokenizer.json`. | `./model/tokenizer.json` |

### Runtime — `persian_normalizer` (Dictionaries)

| Variable | Description | Default / Example |
| :--- | :--- | :--- |
| `G2P_CMUDICT` | Path to the CMU Pronouncing Dictionary file for English-to-Persian transliteration. | `./dict/cmudict.txt` (or beside executable) |
| `G2P_LATIN_DICT` | Path to the custom Latin-to-Persian pronunciation dictionary. | `./dict/latin_dict.txt` (or beside executable) |

### Build-Time (`cargo` & `build.sh`)

| Variable | Description | Default / Example |
| :--- | :--- | :--- |
| `PTTS_LIB_DIR` | Directory containing compiled `libptts_ffi.a` when building `speak --features phonon`. | `./xn-ptts/target/release` |
| `HF_USERNAME` | Hugging Face username used by `build.sh` to clone the gated model repo. | Prompted interactively (optional) |
| `HF_TOKEN` | Hugging Face access token used by `build.sh` to clone the gated model repo. | Prompted interactively (optional) |
| `CLEAN_SAFETENSORS` | Set to `1` in `build.sh` to delete `model.safetensors` automatically after `q8` quantization. | Prompted interactively (`0` / `1`) |

---

## C / C++ / FFI Integration

Headers are located in `include/` and compiled libraries in `target/release/`.

### Dynamic linking (`.so`)

```bash
clang++ main.cpp \
  -I./include \
  -L./target/release \
  -lpersian_normalizer -ltokenizer \
  -Wl,-rpath,'$ORIGIN/target/release' \
  -o app
```

### Static linking (`.a`)

```bash
# Single library
clang++ main.cpp -I./include ./target/release/libpersian_normalizer.a -ldl -lm -lpthread -o app

# Both Rust staticlibs together (allow duplicate Rust std symbols)
clang++ main.cpp -I./include \
  ./target/release/libpersian_normalizer.a \
  ./target/release/libtokenizer.a \
  -Wl,--allow-multiple-definition -ldl -lm -lpthread -o app
```

---

## Licenses & Third-Party Notices

This repository cleanly separates original source code from external dependencies and model weights:

1. **Project Source Code (`crates/`, `include/`, `build.sh`, `dict/latin_dict.txt`)**:
   Licensed under the **MIT License** (see [`LICENSE`](LICENSE)).
2. **TTS Inference Engine (`xn-ptts` submodule)**:
   Developed by [gradium-ai/xn-ptts](https://github.com/gradium-ai/xn-ptts). Subject to the license inside the `xn-ptts/` submodule.
3. **CMU Pronouncing Dictionary (`dict/cmudict.txt`)**:
   Derived from the Carnegie Mellon University Pronouncing Dictionary, distributed under the permissive **BSD-2-Clause** license.
4. **Persian Phonon Model Weights (`mehdi-hf/cilla-mini`)**:
   **Not included in this repository.** The gated model weights on Hugging Face are licensed under **CC-BY-NC-4.0** (Attribution-NonCommercial 4.0 International) and must be obtained directly from Hugging Face by the end user under its original terms.
