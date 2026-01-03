use crate::Context;
use crate::Error;
use hound::{WavSpec, WavWriter};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use tracing::info;

const SAMPLE_RATE: u32 = 48000;
const SAMPLES_PER_FRAME: usize = 960;

#[derive(Debug)]
struct AudioFrame {
    tick_index: u64,
    samples: Vec<i16>,
}

fn parse_log_file(path: &PathBuf) -> Result<Vec<AudioFrame>, Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut frames = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.splitn(2, ' ');
        let tick_str = parts.next().ok_or("Missing tick index")?;
        let samples_str = parts.next().ok_or("Missing samples")?;

        let tick_index: u64 = tick_str.parse()?;
        let samples: Vec<i16> = samples_str
            .split(',')
            .map(|s| s.trim().parse::<i16>())
            .collect::<Result<Vec<_>, _>>()?;

        frames.push(AudioFrame { tick_index, samples });
    }

    Ok(frames)
}

fn load_user_audio(user_dir: &PathBuf) -> Result<BTreeMap<u64, Vec<i16>>, Box<dyn std::error::Error + Send + Sync>> {
    let mut all_frames: BTreeMap<u64, Vec<i16>> = BTreeMap::new();

    // Find all chunk-*.log files
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

    // Sort by chunk number
    chunk_files.sort_by(|a, b| {
        let get_num = |p: &PathBuf| -> u32 {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("chunk-"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };
        get_num(a).cmp(&get_num(b))
    });

    for chunk_path in chunk_files {
        info!("Loading {:?}", chunk_path);
        let frames = parse_log_file(&chunk_path)?;
        for frame in frames {
            all_frames.insert(frame.tick_index, frame.samples);
        }
    }

    Ok(all_frames)
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
        let samples = frames.get(&tick).unwrap_or(&silence);
        for &sample in samples {
            writer.write_sample(sample)?;
        }
    }

    writer.finalize()?;
    Ok(())
}

fn merge_wavs(
    user_audio: &[(String, BTreeMap<u64, Vec<i16>>, u64)],
    output_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if user_audio.is_empty() {
        return Err("No audio to merge".into());
    }

    // Find the earliest first tick and latest last tick across all users
    let earliest_first_tick = user_audio
        .iter()
        .map(|(_, _, first_tick)| *first_tick)
        .min()
        .unwrap();
    
    let latest_last_tick = user_audio
        .iter()
        .map(|(_, frames, _)| {
            frames.keys().next_back().copied().unwrap_or(0)
        })
        .max()
        .unwrap();

    if latest_last_tick < earliest_first_tick {
        return Err("Invalid tick range".into());
    }

    info!(
        "Merging {} users from tick {} to {}",
        user_audio.len(),
        earliest_first_tick,
        latest_last_tick
    );

    let spec = WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(output_path, spec)?;
    let silence = vec![0i16; SAMPLES_PER_FRAME];

    // For each tick, mix all users' audio
    for tick in earliest_first_tick..=latest_last_tick {
        let mut mixed_samples = vec![0i32; SAMPLES_PER_FRAME];

        // Sum all users' samples at this tick
        for (ssrc, frames, first_tick) in user_audio {
            // Only include this user's audio if this tick is >= their first tick
            if tick >= *first_tick {
                if let Some(samples) = frames.get(&tick) {
                    for (i, &sample) in samples.iter().enumerate() {
                        if i < mixed_samples.len() {
                            mixed_samples[i] += sample as i32;
                        }
                    }
                }
            }
        }

        // Convert mixed samples to i16 with clipping
        for mixed in mixed_samples {
            let clipped = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            writer.write_sample(clipped)?;
        }
    }

    writer.finalize()?;
    Ok(())
}

/// Reconstruct audio from a recording session directory
#[poise::command(prefix_command, slash_command, rename = "reconstruct-audio")]
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
    let mut user_audio_data: Vec<(String, BTreeMap<u64, Vec<i16>>, u64)> = Vec::new();

    for user_dir in &user_dirs {
        let ssrc = user_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        info!("Processing user {} (SSRC: {})", ssrc, ssrc);

        match load_user_audio(user_dir) {
            Ok(frames) => {
                if frames.is_empty() {
                    info!("No frames found for user {}", ssrc);
                    continue;
                }

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
                        
                        user_audio_data.push((ssrc.clone(), frames, first_tick));
                    }
                    Err(e) => {
                        errors.push(format!("Failed to write WAV for {}: {}", ssrc, e));
                    }
                }
            }
            Err(e) => {
                errors.push(format!("Failed to load audio for {}: {}", ssrc, e));
            }
        }
    }

    if !user_audio_data.is_empty() {
        let merged_path = output_dir.join("merged.wav");
        match merge_wavs(&user_audio_data, &merged_path) {
            Ok(_) => {
                info!("Created merged WAV: {:?}", merged_path);
            }
            Err(e) => {
                errors.push(format!("Failed to merge WAVs: {}", e));
            }
        }
    }

    let mut response = format!(
        "Reconstructed audio for {} user(s)\nOutput: `{:?}`",
        processed, output_dir
    );

    if !errors.is_empty() {
        response.push_str(&format!("\nErrors:\n{}", errors.join("\n")));
    }

    ctx.say(response).await?;
    Ok(())
}
