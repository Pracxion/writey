use crate::Context;
use crate::Error;
use hound::{WavSpec, WavWriter};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tracing::info;

const SAMPLE_RATE: u32 = 48000;
const SAMPLES_PER_FRAME: usize = 960;

type UserAudio = (String, BTreeMap<u64, Vec<i16>>, u64);

fn load_user_chunks(
    user_dir: &Path,
) -> Result<BTreeMap<u64, Vec<i16>>, Box<dyn std::error::Error + Send + Sync>> {
    let mut chunk_files: Vec<PathBuf> = fs::read_dir(user_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("chunk-") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();

    chunk_files.sort_by_key(|p| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("chunk-"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    });

    let mut frames: BTreeMap<u64, Vec<i16>> = BTreeMap::new();

    for path in chunk_files {
        let file = File::open(&path)?;
        for (i, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let mut parts = line.splitn(2, ' ');
            let tick: u64 = parts
                .next()
                .ok_or_else(|| format!("Missing tick at line {}", i + 1))?
                .parse()
                .map_err(|_| format!("Invalid tick at line {}", i + 1))?;
            let samples: Vec<i16> = parts
                .next()
                .ok_or_else(|| format!("Missing samples at line {}", i + 1))?
                .split(',')
                .map(|s| s.trim().parse::<i16>())
                .collect::<Result<_, _>>()
                .map_err(|_| format!("Invalid sample data at line {}", i + 1))?;

            frames.insert(tick, samples);
        }
    }

    Ok(frames)
}

fn write_wav(
    frames: &BTreeMap<u64, Vec<i16>>,
    output_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if frames.is_empty() {
        return Err("No frames to write".into());
    }

    let spec = WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(output_path, spec)?;
    let first_tick = *frames.keys().next().unwrap();
    let last_tick = *frames.keys().next_back().unwrap();

    info!(
        "Writing WAV from tick {} to {} ({} unique frames)",
        first_tick,
        last_tick,
        frames.len()
    );

    let silence = vec![0i16; SAMPLES_PER_FRAME];
    for tick in first_tick..=last_tick {
        for &s in frames.get(&tick).unwrap_or(&silence) {
            writer.write_sample(s)?;
        }
    }

    writer.finalize()?;
    Ok(())
}

fn merge_wavs(
    user_audio: &[UserAudio],
    output_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if user_audio.is_empty() {
        return Err("No audio to merge".into());
    }

    let earliest = user_audio.iter().map(|(_, _, t)| *t).min().unwrap();
    let latest = user_audio
        .iter()
        .map(|(_, f, _)| f.keys().next_back().copied().unwrap_or(0))
        .max()
        .unwrap();

    if latest < earliest {
        return Err("Invalid tick range".into());
    }

    info!(
        "Merging {} users from tick {} to {}",
        user_audio.len(),
        earliest,
        latest
    );

    let spec = WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(output_path, spec)?;
    let silence = vec![0i16; SAMPLES_PER_FRAME];

    for tick in earliest..=latest {
        let mut mixed = vec![0i32; SAMPLES_PER_FRAME];
        for (_, frames, first_tick) in user_audio {
            if tick >= *first_tick {
                for (i, &s) in frames.get(&tick).unwrap_or(&silence).iter().enumerate() {
                    if i < mixed.len() {
                        mixed[i] += s as i32;
                    }
                }
            }
        }
        for s in mixed {
            writer.write_sample(s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)?;
        }
    }

    writer.finalize()?;
    Ok(())
}

/// Reconstruct audio from a recording session directory
#[poise::command(
    prefix_command,
    slash_command,
    rename = "reconstruct-audio",
    check = "crate::command::is_authorized"
)]
pub async fn reconstruct_audio(
    ctx: Context<'_>,
    #[description = "Session directory path (e.g. recordings/715908438760357910/2026_01_03_18_49_53)"]
    session_dir: String,
) -> Result<(), Error> {
    ctx.defer().await?;

    let session_path = PathBuf::from(&session_dir);
    if !session_path.exists() {
        ctx.say(format!("Session directory not found: {}", session_dir))
            .await?;
        return Ok(());
    }

    let users_dir = session_path.join("users");
    if !users_dir.exists() {
        ctx.say("No users directory found in session").await?;
        return Ok(());
    }

    let output_dir = session_path.join("output");
    fs::create_dir_all(&output_dir)?;

    let user_dirs: Vec<PathBuf> = fs::read_dir(&users_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let mut processed = 0;
    let mut errors = Vec::new();
    let mut user_audio_data: Vec<UserAudio> = Vec::new();

    for user_dir in &user_dirs {
        let ssrc = user_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        match load_user_chunks(user_dir) {
            Ok(frames) if !frames.is_empty() => {
                let first_tick = *frames.keys().next().unwrap();
                let output_path = output_dir.join(format!("{}.wav", ssrc));

                match write_wav(&frames, &output_path) {
                    Ok(_) => {
                        let duration_secs =
                            (frames.len() * SAMPLES_PER_FRAME) as f64 / SAMPLE_RATE as f64;
                        info!(
                            "Created {:?} ({:.1}s, {} frames, first tick: {})",
                            output_path,
                            duration_secs,
                            frames.len(),
                            first_tick
                        );
                        processed += 1;
                        user_audio_data.push((ssrc, frames, first_tick));
                    }
                    Err(e) => errors.push(format!("Failed to write WAV for {}: {}", ssrc, e)),
                }
            }
            Ok(_) => info!("No frames found for SSRC {}", ssrc),
            Err(e) => errors.push(format!("Failed to load audio for {}: {}", ssrc, e)),
        }
    }

    if !user_audio_data.is_empty() {
        let merged_path = output_dir.join("merged.wav");
        if let Err(e) = merge_wavs(&user_audio_data, &merged_path) {
            errors.push(format!("Failed to merge WAVs: {}", e));
        } else {
            info!("Created merged WAV: {:?}", merged_path);
        }
    }

    let mut response = format!(
        "Reconstructed audio for {} user(s)\nOutput: `{}`",
        processed,
        output_dir.display()
    );
    if !errors.is_empty() {
        response.push_str(&format!("\nErrors:\n{}", errors.join("\n")));
    }

    ctx.say(response).await?;
    Ok(())
}
