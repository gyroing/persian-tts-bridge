#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${CYAN}[INFO]${NC}$*"; }
ok()    { echo -e "${GREEN}[OK]${NC}$*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}$*"; }
err()   { echo -e "${RED}[ERROR]${NC}$*" >&2; exit 1; }

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODEL_DIR="$ROOT_DIR/model"
CILLA_DIR="$ROOT_DIR/cilla-mini"
XN_PTTS_DIR="$ROOT_DIR/xn-ptts"
RELEASE_DIR="$ROOT_DIR/target/release"
INCLUDE_DIR="$ROOT_DIR/include"

model_ready() {
    [ -d "$MODEL_DIR" ] && { [ -f "$MODEL_DIR/model.q8.gguf" ] || [ -f "$MODEL_DIR/model.safetensors" ]; }
}

# 0. Check prerequisites
info "Checking prerequisites..."
for cmd in git cargo rustc; do
    command -v "$cmd" >/dev/null 2>&1 || err "Required tool '$cmd' is not installed."
done
[ -d "$ROOT_DIR/crates" ] || err "Workspace '$ROOT_DIR/crates' not found."
ok "Prerequisites OK."

# 1. Model & Hugging Face check
echo ""
echo -e "${BOLD}=== Step 1: Phonon TTS Model ===${NC}"

if model_ready; then
    ok "Model folder already ready at $MODEL_DIR. Skipping download."
else
    if [ -d "$CILLA_DIR" ]; then
        info "Found existing git repo at '$CILLA_DIR'. Skipping clone."
    else
        info "Model folder not found and '$CILLA_DIR' does not exist."
        info "Leave username or token empty to skip Hugging Face download."
        if [ -z "${HF_USERNAME:-}" ]; then
            read -rp "Hugging Face Username (Enter to skip): " HF_USERNAME || true
        fi
        if [ -n "${HF_USERNAME:-}" ] && [ -z "${HF_TOKEN:-}" ]; then
            read -rsp "Hugging Face Token (hidden, Enter to skip): " HF_TOKEN || true
            echo ""
        fi

        if [ -n "${HF_USERNAME:-}" ] && [ -n "${HF_TOKEN:-}" ]; then
            info "Cloning mehdi-hf/cilla-mini..."
            git clone "https://${HF_USERNAME}:${HF_TOKEN}@huggingface.co/mehdi-hf/cilla-mini" "$CILLA_DIR"
            ok "Cloned cilla-mini."
        else
            warn "No Hugging Face token provided. Skipping model download."
        fi
    fi

    if [ -d "$CILLA_DIR" ]; then
        if command -v git-lfs >/dev/null 2>&1 && [ -d "$CILLA_DIR/.git" ]; then
            info "Pulling Git LFS objects..."
            (cd "$CILLA_DIR" && git lfs install && git lfs pull)
        fi

        if [ -d "$CILLA_DIR/phonon/model" ]; then
            mkdir -p "$MODEL_DIR"
            info "Copying model files to $MODEL_DIR..."
            cp -rL "$CILLA_DIR/phonon/model/." "$MODEL_DIR/"
            if [ -f "$CILLA_DIR/phonon/web/timing_model.json" ]; then
                cp -L "$CILLA_DIR/phonon/web/timing_model.json" "$MODEL_DIR/"
                ok "Copied timing_model.json to $MODEL_DIR/"
            else
                warn "timing_model.json not found in $CILLA_DIR/phonon/web/"
            fi
            ok "Model assets ready in $MODEL_DIR/"
        else
            warn "Missing $CILLA_DIR/phonon/model; cannot populate$MODEL_DIR."
        fi
    fi
fi

# 2. Setup xn-ptts (submodule first, fallback to clone)
echo ""
echo -e "${BOLD}=== Step 2: Setup xn-ptts ===${NC}"
if [ ! -f "$XN_PTTS_DIR/Cargo.toml" ]; then
    if [ -d "$ROOT_DIR/.git" ] && [ -f "$ROOT_DIR/.gitmodules" ]; then
        info "Initializing xn-ptts git submodule..."
        (cd "$ROOT_DIR" && git submodule update --init --recursive xn-ptts)
        ok "Initialized xn-ptts submodule."
    else
        info "Cloning gradium-ai/xn-ptts..."
        git clone https://github.com/gradium-ai/xn-ptts "$XN_PTTS_DIR"
        ok "Cloned xn-ptts."
    fi
else
    ok "Directory '$XN_PTTS_DIR' is ready."
fi

# 3. Quantize model.safetensors -> model.q8.gguf
echo ""
echo -e "${BOLD}=== Step 3: Quantize model (q8) ===${NC}"
SAFETENSORS_FILE="$MODEL_DIR/model.safetensors"
GGUF_FILE="$MODEL_DIR/model.q8.gguf"

if [ -f "$GGUF_FILE" ]; then
    ok "$GGUF_FILE already exists. Skipping quantization."
elif [ -f "$SAFETENSORS_FILE" ]; then
    info "Running quantize (q8, --no-mimi-encoder)..."
    (
        cd "$XN_PTTS_DIR"
        cargo run --release --example quantize -- \
            "$SAFETENSORS_FILE" \
            "$GGUF_FILE" \
            --quant q8 \
            --no-mimi-encoder
    )
    ok "Quantized model saved to $GGUF_FILE"
else
    warn "Neither model.q8.gguf nor model.safetensors found in $MODEL_DIR. Skipping quantization."
fi

if [ -f "$SAFETENSORS_FILE" ] && [ -f "$GGUF_FILE" ]; then
    if [ "${CLEAN_SAFETENSORS:-}" = "1" ]; then
        rm -f "$SAFETENSORS_FILE"
        ok "Removed $SAFETENSORS_FILE."
    else
        read -rp "Delete original model.safetensors to save disk space? [y/N]: " ans || true
        if [[ "${ans:-}" =~ ^[Yy]$ ]]; then
            rm -f "$SAFETENSORS_FILE"
            ok "Removed $SAFETENSORS_FILE."
        else
            info "Kept $SAFETENSORS_FILE."
        fi
    fi
fi

# 4. Build ptts-ffi
echo ""
echo -e "${BOLD}=== Step 4: Build ptts-ffi ===${NC}"
info "Building libptts_ffi (release)..."
(
    cd "$XN_PTTS_DIR"
    cargo build --release -p ptts-ffi
)
ok "Built ptts-ffi."

# 5. Build Workspace in standard target/release
echo ""
echo -e "${BOLD}=== Step 5: Build Workspace (.a/.so & speak) ===${NC}"
(
    cd "$ROOT_DIR"

    info "Building persian_normalizer & tokenizer (.a/.so)..."
    cargo build --release -p persian_normalizer -p tokenizer

    info "Building speak (static link with ptts_ffi)..."
    PTTS_LIB_DIR="$XN_PTTS_DIR/target/release" \
        cargo build --release -p speak --features phonon
)
ok "Build artifacts ready in $RELEASE_DIR"

echo ""
echo -e "${GREEN}${BOLD}=== Build Complete! ===${NC}"
ls -lh \
    "$RELEASE_DIR/speak" \
    "$RELEASE_DIR"/libpersian_normalizer.{a,so} \
    "$RELEASE_DIR"/libtokenizer.{a,so} \
    "$INCLUDE_DIR"/*.h 2>/dev/null || true
echo ""
echo "Test from $ROOT_DIR:"
echo "  echo \"سلام دنیا\" | ./target/release/speak | pacat --raw --format=s16le --rate=24000 --channels=1"
echo "  termux-clipboard-get | ./target/release/speak | pacat --raw --format=s16le --rate=24000 --channels=1"
