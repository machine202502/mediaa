use anyhow::Context;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

fn parse_chunks<'a>(bytes: &'a [u8], source: &Path) -> anyhow::Result<Vec<Chunk<'a>>> {
    anyhow::ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "{} is not a WebP file",
        source.display()
    );
    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")) as usize;
    let end = riff_size
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .with_context(|| format!("{} has a truncated RIFF header", source.display()))?;
    let mut chunks = Vec::new();
    let mut offset = 12;
    while offset < end {
        anyhow::ensure!(
            end - offset >= 8,
            "{} has a truncated WebP chunk",
            source.display()
        );
        let id = bytes[offset..offset + 4].try_into().expect("four bytes");
        let size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("four bytes"),
        ) as usize;
        let data_start = offset + 8;
        let data_end = data_start
            .checked_add(size)
            .filter(|data_end| *data_end <= end)
            .with_context(|| format!("{} has a truncated WebP payload", source.display()))?;
        chunks.push(Chunk {
            id,
            data: &bytes[data_start..data_end],
        });
        offset = data_end + (size & 1);
    }
    anyhow::ensure!(
        offset == end,
        "{} has malformed WebP padding",
        source.display()
    );
    Ok(chunks)
}

fn chunk<'a>(chunks: &'a [Chunk<'a>], id: &[u8; 4]) -> Option<&'a [u8]> {
    chunks
        .iter()
        .find(|chunk| &chunk.id == id)
        .map(|chunk| chunk.data)
}

fn canvas(chunks: &[Chunk<'_>], source: &Path) -> anyhow::Result<[u8; 6]> {
    let data = chunk(chunks, b"VP8X")
        .with_context(|| format!("{} has no VP8X chunk", source.display()))?;
    anyhow::ensure!(
        data.len() == 10 && data[0] & 0x02 != 0,
        "{} is not an animated WebP",
        source.display()
    );
    Ok(data[4..10].try_into().expect("six bytes"))
}

fn animation(chunks: &[Chunk<'_>], source: &Path) -> anyhow::Result<[u8; 6]> {
    let data = chunk(chunks, b"ANIM")
        .with_context(|| format!("{} has no ANIM chunk", source.display()))?;
    anyhow::ensure!(
        data.len() == 6,
        "{} has an invalid ANIM chunk",
        source.display()
    );
    Ok(data.try_into().expect("six bytes"))
}

fn write_chunk(file: &mut File, id: &[u8; 4], data: &[u8]) -> anyhow::Result<()> {
    let size = u32::try_from(data.len()).context("WebP chunk is too large")?;
    file.write_all(id)?;
    file.write_all(&size.to_le_bytes())?;
    file.write_all(data)?;
    if data.len() & 1 != 0 {
        file.write_all(&[0])?;
    }
    Ok(())
}

fn write_chunk_bytes(output: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) -> anyhow::Result<()> {
    let size = u32::try_from(data.len()).context("WebP chunk is too large")?;
    output.extend_from_slice(id);
    output.extend_from_slice(&size.to_le_bytes());
    output.extend_from_slice(data);
    if data.len() & 1 != 0 {
        output.push(0);
    }
    Ok(())
}

fn write_u24(output: &mut [u8], value: u32) {
    output[0] = value as u8;
    output[1] = (value >> 8) as u8;
    output[2] = (value >> 16) as u8;
}

fn bitstream_dimensions(chunks: &[Chunk<'_>], source: &Path) -> anyhow::Result<(u32, u32)> {
    if let Some(data) = chunk(chunks, b"VP8 ") {
        anyhow::ensure!(
            data.len() >= 10 && data[3..6] == [0x9d, 0x01, 0x2a],
            "{} has an invalid VP8 frame header",
            source.display()
        );
        let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
        let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
        return Ok((width.max(1), height.max(1)));
    }
    if let Some(data) = chunk(chunks, b"VP8L") {
        anyhow::ensure!(
            data.len() >= 5 && data[0] == 0x2f,
            "{} has an invalid VP8L frame header",
            source.display()
        );
        let bits = u32::from_le_bytes(data[1..5].try_into().expect("four bytes"));
        return Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1));
    }
    anyhow::bail!("{} has no VP8 or VP8L image chunk", source.display())
}

fn has_alpha(chunks: &[Chunk<'_>]) -> bool {
    if chunks.iter().any(|chunk| &chunk.id == b"ALPH") {
        return true;
    }
    chunk(chunks, b"VP8L").is_some_and(|data| {
        data.len() >= 5 && u32::from_le_bytes([data[1], data[2], data[3], data[4]]) & (1 << 28) != 0
    })
}

fn frame_payload(
    chunks: &[Chunk<'_>],
    width: u32,
    height: u32,
    duration_ms: u32,
) -> anyhow::Result<Vec<u8>> {
    let mut payload = vec![0u8; 16];
    write_u24(&mut payload[6..9], width.saturating_sub(1));
    write_u24(&mut payload[9..12], height.saturating_sub(1));
    write_u24(&mut payload[12..15], duration_ms);
    payload[15] = 0x02;
    let mut frame = Vec::new();
    for chunk in chunks {
        if matches!(&chunk.id, b"ALPH" | b"VP8 " | b"VP8L") {
            write_chunk_bytes(&mut frame, &chunk.id, chunk.data)?;
        }
    }
    anyhow::ensure!(!frame.is_empty(), "static WebP has no frame payload");
    payload.extend_from_slice(&frame);
    Ok(payload)
}

pub(crate) fn normalize_animation(path: &Path, duration_us: i64) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let chunks = parse_chunks(&bytes, path)?;
    if chunks.iter().any(|chunk| &chunk.id == b"ANMF") {
        return Ok(());
    }
    let (width, height) = bitstream_dimensions(&chunks, path)?;
    let duration_ms = u32::try_from(duration_us.max(0).saturating_add(999) / 1_000)
        .unwrap_or(u32::MAX)
        .clamp(1, 0xff_ffff);
    let mut canvas = [0u8; 10];
    canvas[0] = 0x02 | if has_alpha(&chunks) { 0x10 } else { 0 };
    write_u24(&mut canvas[4..7], width.saturating_sub(1));
    write_u24(&mut canvas[7..10], height.saturating_sub(1));
    let animation = [0u8; 6];
    let frame = frame_payload(&chunks, width, height, duration_ms)?;
    let mut output = Vec::new();
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(b"WEBP");
    write_chunk_bytes(&mut output, b"VP8X", &canvas)?;
    write_chunk_bytes(&mut output, b"ANIM", &animation)?;
    write_chunk_bytes(&mut output, b"ANMF", &frame)?;
    let riff_size = u32::try_from(output.len() - 8).context("normalized WebP is too large")?;
    output[4..8].copy_from_slice(&riff_size.to_le_bytes());
    std::fs::write(path, output).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn validate_animation(path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let chunks = parse_chunks(&bytes, path)?;
    canvas(&chunks, path)?;
    animation(&chunks, path)?;
    anyhow::ensure!(
        chunks.iter().any(|chunk| &chunk.id == b"ANMF"),
        "{} has no animation frames",
        path.display()
    );
    Ok(())
}

pub(crate) fn trim_animation(path: &Path, max_duration_us: i64) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let chunks = parse_chunks(&bytes, path)?;
    let max_ms = u32::try_from(max_duration_us.max(0).saturating_add(999) / 1_000)
        .unwrap_or(u32::MAX)
        .max(1);
    let mut total = 0u32;
    let mut kept: Vec<Vec<u8>> = Vec::new();
    for chunk in &chunks {
        if &chunk.id != b"ANMF" {
            continue;
        }
        if total >= max_ms {
            break;
        }
        let duration = {
            let bytes = &chunk.data[12..15];
            u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
        };
        let remaining = max_ms - total;
        if duration > remaining {
            let mut frame = chunk.data.to_vec();
            write_u24(&mut frame[12..15], remaining);
            kept.push(frame);
            break;
        }
        kept.push(chunk.data.to_vec());
        total = total.saturating_add(duration);
    }
    anyhow::ensure!(
        !kept.is_empty(),
        "{} has no animation frames",
        path.display()
    );
    let mut output = Vec::new();
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(b"WEBP");
    for chunk in &chunks {
        if &chunk.id != b"ANMF" {
            write_chunk_bytes(&mut output, &chunk.id, chunk.data)?;
        }
    }
    for frame in &kept {
        write_chunk_bytes(&mut output, b"ANMF", frame)?;
    }
    let riff_size = u32::try_from(output.len() - 8).context("trimmed WebP is too large")?;
    output[4..8].copy_from_slice(&riff_size.to_le_bytes());
    std::fs::write(path, output).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn merge_animation(parts: &[PathBuf], output: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(!parts.is_empty(), "no WebP segments to merge");
    let first = std::fs::read(&parts[0]).with_context(|| format!("read {}", parts[0].display()))?;
    let first_chunks = parse_chunks(&first, &parts[0])?;
    let expected_canvas = canvas(&first_chunks, &parts[0])?;
    let expected_animation = animation(&first_chunks, &parts[0])?;
    anyhow::ensure!(
        first_chunks.iter().any(|chunk| &chunk.id == b"ANMF"),
        "{} has no animation frames",
        parts[0].display()
    );

    let mut file = File::create(output).with_context(|| format!("create {}", output.display()))?;
    file.write_all(b"RIFF")?;
    file.write_all(&[0; 4])?;
    file.write_all(b"WEBP")?;
    for chunk in &first_chunks {
        if &chunk.id != b"ANMF" {
            write_chunk(&mut file, &chunk.id, chunk.data)?;
        }
    }
    for chunk in &first_chunks {
        if &chunk.id == b"ANMF" {
            write_chunk(&mut file, &chunk.id, chunk.data)?;
        }
    }

    for path in &parts[1..] {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let chunks = parse_chunks(&data, path)?;
        anyhow::ensure!(
            canvas(&chunks, path)? == expected_canvas,
            "WebP segment sizes differ: {}",
            path.display()
        );
        anyhow::ensure!(
            animation(&chunks, path)? == expected_animation,
            "WebP animation settings differ: {}",
            path.display()
        );
        let mut frames = 0;
        for chunk in &chunks {
            if &chunk.id == b"ANMF" {
                write_chunk(&mut file, &chunk.id, chunk.data)?;
                frames += 1;
            }
        }
        anyhow::ensure!(frames > 0, "{} has no animation frames", path.display());
    }

    let total = file.stream_position()?;
    let riff_size = u32::try_from(total - 8).context("merged WebP is too large")?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn synthetic_animation(frame: u8) -> Vec<u8> {
    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if data.len() & 1 != 0 {
            bytes.push(0);
        }
        bytes
    }
    let mut chunks = Vec::new();
    chunks.extend(chunk(b"VP8X", &[0x02, 0, 0, 0, 9, 0, 0, 10, 0, 0]));
    chunks.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    chunks.extend(chunk(b"ANMF", &[frame; 17]));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&chunks);
    bytes
}

#[cfg(test)]
pub(crate) fn synthetic_still(width: u32, height: u32, frame: u8) -> Vec<u8> {
    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if data.len() & 1 != 0 {
            bytes.push(0);
        }
        bytes
    }
    let mut vp8 = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
    vp8.extend_from_slice(&((width - 1) as u16).to_le_bytes());
    vp8.extend_from_slice(&((height - 1) as u16).to_le_bytes());
    vp8.push(frame);
    let chunks = chunk(b"VP8 ", &vp8);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&chunks);
    bytes
}

#[cfg(test)]
pub(crate) fn synthetic_transparent_still(width: u32, height: u32, frame: u8) -> Vec<u8> {
    let still = synthetic_still(width, height, frame);
    let mut alpha = Vec::new();
    alpha.extend_from_slice(b"ALPH");
    alpha.extend_from_slice(&2u32.to_le_bytes());
    alpha.extend_from_slice(&[0, 0]);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((still.len() - 8 + alpha.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&alpha);
    bytes.extend_from_slice(&still[12..]);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_animation_keeps_only_the_common_duration() {
        fn animation_with_duration(duration_ms: u32) -> Vec<u8> {
            let mut bytes = synthetic_animation(0);
            let at = bytes
                .windows(4)
                .position(|window| window == b"ANMF")
                .expect("ANMF")
                + 8;
            bytes[at + 12] = duration_ms as u8;
            bytes[at + 13] = (duration_ms >> 8) as u8;
            bytes[at + 14] = (duration_ms >> 16) as u8;
            bytes
        }

        let dir = tempfile::tempdir().unwrap();
        let parts: Vec<_> = (0..3)
            .map(|i| {
                let path = dir.path().join(format!("part-{i}.webp"));
                std::fs::write(&path, animation_with_duration(100)).unwrap();
                path
            })
            .collect();
        let output = dir.path().join("merged.webp");
        merge_animation(&parts, &output).unwrap();
        let before = std::fs::read(&output).unwrap();
        for chunk in parse_chunks(&before, &output).unwrap() {
            if &chunk.id == b"ANMF" {
                assert!(chunk.data.len() >= 15, "frame len {}", chunk.data.len());
            }
        }
        trim_animation(&output, 150_000).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let chunks = parse_chunks(&bytes, &output).unwrap();
        let frames: Vec<_> = chunks.iter().filter(|chunk| &chunk.id == b"ANMF").collect();
        assert_eq!(frames.len(), 2);
        let duration = &frames[1].data[12..15];
        assert_eq!(
            u32::from(duration[0]) | (u32::from(duration[1]) << 8) | (u32::from(duration[2]) << 16),
            50
        );
    }

    #[test]
    fn merges_frames_and_keeps_one_animation_header() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("part-0.webp");
        let second = dir.path().join("part-1.webp");
        let output = dir.path().join("merged.webp");
        std::fs::write(&first, synthetic_animation(1)).unwrap();
        std::fs::write(&second, synthetic_animation(2)).unwrap();
        merge_animation(&[first, second], &output).unwrap();
        let bytes = std::fs::read(output).unwrap();
        let chunks = parse_chunks(&bytes, Path::new("merged.webp")).unwrap();
        assert_eq!(
            chunks.iter().filter(|chunk| &chunk.id == b"VP8X").count(),
            1
        );
        assert_eq!(
            chunks.iter().filter(|chunk| &chunk.id == b"ANIM").count(),
            1
        );
        assert_eq!(
            chunks.iter().filter(|chunk| &chunk.id == b"ANMF").count(),
            2
        );
    }

    #[test]
    fn rejects_mismatched_canvas_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("part-0.webp");
        let second = dir.path().join("part-1.webp");
        let output = dir.path().join("merged.webp");
        std::fs::write(&first, synthetic_animation(1)).unwrap();
        let mut other = synthetic_animation(2);
        other[24] = 8;
        std::fs::write(&second, other).unwrap();
        let error = merge_animation(&[first, second], &output).unwrap_err();
        assert!(format!("{error:#}").contains("sizes differ"));
    }

    #[test]
    fn static_webp_becomes_a_timed_animation_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("static.webp");
        std::fs::write(&path, synthetic_still(9, 10, 7)).unwrap();
        normalize_animation(&path, 1_250_000).unwrap();
        validate_animation(&path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let chunks = parse_chunks(&bytes, Path::new("static.webp")).unwrap();
        let frame = chunks.iter().find(|chunk| &chunk.id == b"ANMF").unwrap();
        assert_eq!(
            u32::from_le_bytes([frame.data[12], frame.data[13], frame.data[14], 0]),
            1_250
        );
        assert_eq!(&frame.data[16..20], b"VP8 ");
    }

    #[test]
    fn normalized_static_webp_keeps_the_alpha_capability_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transparent.webp");
        std::fs::write(&path, synthetic_transparent_still(9, 10, 7)).unwrap();
        normalize_animation(&path, 1_000_000).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let chunks = parse_chunks(&bytes, Path::new("transparent.webp")).unwrap();
        let canvas = chunk(&chunks, b"VP8X").unwrap();
        assert_ne!(canvas[0] & 0x10, 0);
    }

    #[test]
    fn normalization_keeps_existing_animation_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("animated.webp");
        let original = synthetic_animation(3);
        std::fs::write(&path, &original).unwrap();
        normalize_animation(&path, 1_000_000).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn malformed_chunk_corpus_returns_errors_without_panicking() {
        let path = Path::new("fuzz.webp");
        for len in 0..256usize {
            let mut state = 0x9e37_79b9u32 ^ len as u32;
            let mut bytes = vec![0u8; len];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let _ = parse_chunks(&bytes, path);
        }
        let mut truncated = synthetic_animation(1);
        truncated.truncate(truncated.len() - 3);
        assert!(parse_chunks(&truncated, path).is_err());
        let mut bad_padding = synthetic_animation(1);
        bad_padding.push(7);
        let riff_size = (bad_padding.len() - 8) as u32;
        bad_padding[4..8].copy_from_slice(&riff_size.to_le_bytes());
        assert!(parse_chunks(&bad_padding, path).is_err());
    }
}
