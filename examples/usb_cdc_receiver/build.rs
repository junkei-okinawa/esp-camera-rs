#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    image_sender_cam1: &'static str,
    #[default("")]
    image_sender_cam2: &'static str,
    #[default("")]
    image_sender_cam3: &'static str,
    #[default("")]
    image_sender_cam4: &'static str,
    #[default("")]
    image_sender_cam5: &'static str,
    #[default("")]
    image_sender_cam6: &'static str,
}

fn main() {
    // Check if the `cfg.toml` file exists
    if !std::path::Path::new("cfg.toml").exists() {
        panic!("You need to create a `cfg.toml` file with camera MAC addresses! Use `cfg.toml.template` as a template.");
    }

    // The constant `CONFIG` is auto-generated by `toml_config`.
    let app_config = CONFIG;

    // 設定済みのカメラアドレスを確認
    let mut found_cameras = 0;

    if !app_config.image_sender_cam1.is_empty() {
        println!("Camera 1: {}", app_config.image_sender_cam1);
        found_cameras += 1;
    }

    if !app_config.image_sender_cam2.is_empty() {
        println!("Camera 2: {}", app_config.image_sender_cam2);
        found_cameras += 1;
    }

    if !app_config.image_sender_cam3.is_empty() {
        println!("Camera 3: {}", app_config.image_sender_cam3);
        found_cameras += 1;
    }

    if !app_config.image_sender_cam4.is_empty() {
        println!("Camera 4: {}", app_config.image_sender_cam4);
        found_cameras += 1;
    }

    if !app_config.image_sender_cam5.is_empty() {
        println!("Camera 5: {}", app_config.image_sender_cam5);
        found_cameras += 1;
    }

    if !app_config.image_sender_cam6.is_empty() {
        println!("Camera 6: {}", app_config.image_sender_cam6);
        found_cameras += 1;
    }

    if found_cameras == 0 {
        println!("Warning: No camera MAC addresses configured in `cfg.toml`.");
    } else {
        println!("Found {} camera MAC addresses in cfg.toml", found_cameras);
    }

    // Make App_config available as a system environment variable.
    embuild::espidf::sysenv::output();
}
