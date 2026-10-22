use std::path::Path;

#[test]
fn shipped_config_is_valid() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/config.toml");
    let config = lard_core::Config::load(&path).expect("config/config.toml must parse");
    config.validate().unwrap();
}
