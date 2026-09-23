fn main() {
    println!("cargo::rustc-check-cfg=cfg(wsp_crash_test)");
    if std::env::var_os("CARGO_FEATURE_TEST_CRASH_BARRIERS").is_some()
        && std::env::var("PROFILE").as_deref() != Ok("debug")
    {
        panic!("test-crash-barriers may only be built with Cargo's debug profile");
    }
}
