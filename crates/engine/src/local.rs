//! Validation of model files on disk: a model is a weights file or a folder holding weights
//! (safetensors, GGUF, ONNX or PyTorch). Safetensors are checked in full (header and every
//! tensor's bytes in range); GGUF and ONNX by their file signature.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};

const WEIGHTS: [&str; 6] = ["safetensors", "gguf", "onnx", "bin", "pt", "pth"];

fn is_weights(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| WEIGHTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Checks one weights file; returns its size.
pub fn check_file(path: &Path) -> anyhow::Result<u64> {
    let size = std::fs::metadata(path).with_context(|| format!("{} not found", path.display()))?.len();
    anyhow::ensure!(size > 0, "{} is empty", path.display());
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "safetensors" => {
            let file = std::fs::File::open(path)?;
            // SAFETY: read-only mapping of a file we do not modify while it is mapped.
            let map = unsafe { memmap2::Mmap::map(&file)? };
            safetensors::SafeTensors::deserialize(&map).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        }
        "gguf" => {
            let mut head = [0u8; 8];
            std::io::Read::read_exact(&mut std::fs::File::open(path)?, &mut head)?;
            anyhow::ensure!(&head[..4] == b"GGUF", "{}: not a GGUF file", path.display());
            let version = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
            anyhow::ensure!((2..=3).contains(&version), "{}: GGUF version {version} is not supported", path.display());
        }
        "onnx" => {
            // ModelProto starts with field 1 (ir_version, varint): tag byte 0x08.
            let mut head = [0u8; 1];
            std::io::Read::read_exact(&mut std::fs::File::open(path)?, &mut head)?;
            anyhow::ensure!(head[0] == 0x08, "{}: not an ONNX model", path.display());
        }
        _ => {}
    }
    Ok(size)
}

/// Checks a model file or folder; returns the weights files and their total size.
pub fn check(path: &Path) -> anyhow::Result<(Vec<PathBuf>, u64)> {
    let files: Vec<PathBuf> = if path.is_dir() {
        let mut found = Vec::new();
        let mut stack = vec![path.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if is_weights(&p) {
                    found.push(p);
                }
            }
        }
        found.sort();
        found
    } else if path.exists() {
        anyhow::ensure!(is_weights(path), "{}: not a model file ({})", path.display(), WEIGHTS.join(", "));
        vec![path.to_path_buf()]
    } else {
        bail!("{} not found", path.display());
    };
    anyhow::ensure!(!files.is_empty(), "{}: no model weights ({})", path.display(), WEIGHTS.join(", "));
    let mut total = 0;
    for f in &files {
        total += check_file(f)?;
    }
    Ok((files, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safetensors_bytes() -> Vec<u8> {
        let data = [0u8; 8];
        let view = safetensors::tensor::TensorView::new(safetensors::Dtype::F32, vec![2], &data).unwrap();
        safetensors::serialize([("w", view)], None).unwrap()
    }

    #[test]
    fn valid_and_broken_weights() {
        let dir = tempfile::tempdir().unwrap();
        let st = dir.path().join("model.safetensors");
        std::fs::write(&st, safetensors_bytes()).unwrap();
        assert!(check_file(&st).is_ok());
        let mut cut = safetensors_bytes();
        cut.truncate(cut.len() - 4);
        std::fs::write(&st, &cut).unwrap();
        assert!(check_file(&st).is_err(), "truncated tensor data");

        let gguf = dir.path().join("m.gguf");
        std::fs::write(&gguf, [b"GGUF".as_slice(), &3u32.to_le_bytes()].concat()).unwrap();
        assert!(check_file(&gguf).is_ok());
        std::fs::write(&gguf, b"NOPE1234").unwrap();
        assert!(check_file(&gguf).is_err());

        let onnx = dir.path().join("m.onnx");
        std::fs::write(&onnx, [0x08, 0x07]).unwrap();
        assert!(check_file(&onnx).is_ok());
        std::fs::write(&onnx, b"<html>").unwrap();
        assert!(check_file(&onnx).is_err(), "an error page saved as a model");
    }

    #[test]
    fn folders_and_missing_models() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check(dir.path()).is_err(), "no weights");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("model.safetensors"), safetensors_bytes()).unwrap();
        std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
        let (files, size) = check(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(size > 0);
        assert!(check(&dir.path().join("config.json")).is_err(), "not weights");
        assert!(check(&dir.path().join("missing.gguf")).is_err());
    }
}
