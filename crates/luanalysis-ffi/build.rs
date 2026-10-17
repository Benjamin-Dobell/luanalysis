//! Names the shared library as it is shipped, so programs linked with it find it at run time:
//! `@rpath/libluanalysis.dylib` on macOS and the soname `libluanalysis.so` on Linux.
fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libluanalysis.dylib"),
        Ok("linux") => println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libluanalysis.so"),
        _ => {}
    }
}
