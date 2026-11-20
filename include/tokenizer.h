#ifndef PTTS_TOKENIZER_H
#define PTTS_TOKENIZER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct PttsTokenizer PttsTokenizer;

/* Load SentencePiece BPE tokenizer from tokenizer.json. Returns NULL on error. */
PttsTokenizer* ptts_tokenizer_new(const char* tokenizer_json_path);
void ptts_tokenizer_free(PttsTokenizer* tok);

/* Count tokens in text. Returns -1 on error. */
int32_t ptts_tokenizer_count(const PttsTokenizer* tok, const char* text);

/* Encode text into token IDs.
 * Copies up to max_len IDs into out_ids and returns total token count (or -1 on error).
 * Pass out_ids = NULL (or max_len = 0) to query the required buffer length first. */
int32_t ptts_tokenizer_encode(
    const PttsTokenizer* tok,
    const char* text,
    int32_t* out_ids,
    int32_t max_len
);

/* Split text into newline-separated chunks (pass 0 for defaults 80/8).
 * Free the returned string with ptts_tokenizer_string_free(). */
char* ptts_tokenizer_split(
    const PttsTokenizer* tok,
    const char* text,
    int32_t max_tokens,
    int32_t min_tokens
);

void ptts_tokenizer_string_free(char* s);

#ifdef __cplusplus
}
#endif

#endif /* PTTS_TOKENIZER_H */
