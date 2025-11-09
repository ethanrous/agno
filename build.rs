fn main() {
    if !cfg!(feature = "pdf") {
        return;
    }

    // Directory that contains libpdfium.a (not the file itself)
    let lib_dir = std::env::var("PDFIUM_STATIC_LIB_PATH")
        .or_else(|_| std::env::var("PDFIUM_LIB_PATH"))
        .unwrap_or_else(|_| "/agno/libpdfium".to_string());

    // Make rustc find libpdfium.a
    println!("cargo:rustc-link-search=native={}", lib_dir);
    println!("cargo:rustc-link-search=native=/usr/local/musl/lib");
    println!("cargo:rustc-link-search=native=/usr/local/musl/include");

    // Link libpdfium statically
    println!("cargo:rustc-link-lib=static=pdfium");

    // Link the C++ runtime and common system libs (shared is fine)
    // println!("cargo:rustc-link-lib=static=c++");
    // println!("cargo:rustc-link-lib=m");
    // println!("cargo:rustc-link-lib=pthread");
    // println!("cargo:rustc-link-lib=dl");
    // println!("cargo:rustc-link-lib=atomic");

    // Alpine/musl toolchains often enable --as-needed by default, which can drop libs
    // if referenced later. Disable it to avoid surprises:
    println!("cargo:rustc-link-arg=-Wl,--no-as-needed");

    // If you still see unresolved symbols due to static-archive ordering, wrap in a group:
    // println!("cargo:rustc-link-arg=-Wl,--start-group");
    // println!("cargo:rustc-link-lib=static=pdfium");
    // println!("cargo:rustc-link-lib=stdc++");
    // println!("cargo:rustc-link-arg=-Wl,--end-group");
}
