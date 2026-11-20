fn main() {
    println!("cargo:rerun-if-env-changed=PTTS_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_PHONON").is_some() {
        let dir = std::env::var("PTTS_LIB_DIR").unwrap_or_else(|_| ".".to_string());
        println!("cargo:rustc-link-search=native={}", dir);
        println!("cargo:rustc-link-lib=static=ptts_ffi");
        println!("cargo:rustc-link-arg-bin=speak=-Wl,--allow-multiple-definition");
    }
}

