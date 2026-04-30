//! libmpv2 smoke test: can we construct an Mpv handle with a null AO?
//!
//! This verifies `libmpv.so` is present at build + runtime and that
//! `Mpv::with_initializer` actually initializes an mpv core. Setting
//! `ao = null` during init prevents mpv from opening a real audio device
//! in CI / headless environments.

#[test]
fn can_construct_mpv_handle() {
    let mpv = libmpv2::Mpv::with_initializer(|init| {
        init.set_property("vid", "no")?;
        init.set_property("ao", "null")?;
        init.set_property("audio-buffer", 2.0_f64)?;
        Ok(())
    });

    assert!(mpv.is_ok(), "Mpv::with_initializer failed: {:?}", mpv.err());
}
