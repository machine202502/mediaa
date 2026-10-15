//! `ffmpeg-sys-next` links the FFmpeg libraries themselves when `FFMPEG_DIR` is set, but not
//! their transitive dependencies (x264, libwebp, zlib, OS libraries). Those are listed in the
//! `Libs.private` lines of FFmpeg's pkg-config files, so read them from there.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Some(dir) = std::env::var_os("FFMPEG_DIR").map(PathBuf::from) else {
        return;
    };
    let lib_dir = dir.join("lib");
    let pc_dir = lib_dir.join("pkgconfig");
    println!("cargo:rerun-if-changed={}", pc_dir.display());
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    let ffmpeg_libs = [
        "avcodec", "avdevice", "avfilter", "avformat", "avutil", "swresample", "swscale",
    ];
    let mut libs = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(&pc_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            let Some(rest) = line
                .strip_prefix("Libs.private:")
                .or_else(|| line.strip_prefix("Libs:"))
            else {
                continue;
            };
            for flag in rest.split_whitespace() {
                if let Some(name) = flag.strip_prefix("-l") {
                    if !ffmpeg_libs.contains(&name) {
                        libs.insert(name.to_string());
                    }
                }
            }
        }
    }
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    for name in libs {
        let bundled = if windows {
            lib_dir.join(format!("{name}.lib")).is_file()
        } else {
            lib_dir.join(format!("lib{name}.a")).is_file()
        };
        if bundled {
            println!("cargo:rustc-link-lib=static={name}");
        } else {
            println!("cargo:rustc-link-lib={name}");
        }
    }
}
