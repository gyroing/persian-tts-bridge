#ifndef PERSIAN_NORMALIZER_H
#define PERSIAN_NORMALIZER_H

#ifdef __cplusplus
extern "C" {
#endif

/* Normalize Persian/mixed text. Pass NULL for dict paths to use env/defaults.
 * Free the returned string with persian_normalize_free(). */
char* persian_normalize(
    const char* text,
    const char* cmudict_path,
    const char* latin_dict_path
);

void persian_normalize_free(char* s);

#ifdef __cplusplus
}
#endif

#endif /* PERSIAN_NORMALIZER_H */
