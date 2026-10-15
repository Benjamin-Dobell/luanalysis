// Original integration sources remain separate Rust modules. The build script
// discovers Cargo-style test roots automatically; authors maintain no list.
include!(concat!(env!("OUT_DIR"), "/integration_tests.rs"));
