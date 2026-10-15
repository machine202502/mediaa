//! Embeds the app icon into the Windows executable (Explorer, taskbar, Alt+Tab).

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The Vulkan loader comes with GPU drivers; loading it only when models run keeps the app
    // starting on machines without one (`mediaa_agent::llm` checks for it first).
    println!("cargo:rustc-link-arg-bins=/DELAYLOAD:vulkan-1.dll");
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico_path = out.join("mediaa.ico");
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let image = ico::IconImage::from_rgba_data(size, size, icon::rgba(size));
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("encode icon"));
    }
    dir.write(std::fs::File::create(&ico_path).expect("create ico"))
        .expect("write ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("utf-8 path"));
    res.set("FileDescription", "MediaA");
    res.set("ProductName", "MediaA");
    res.compile().expect("compile Windows resources");
}
