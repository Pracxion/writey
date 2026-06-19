use crate::Context;
use crate::Error;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

const PART_SIZE: usize = 300 * 1024 * 1024; // 300 MB

/// Convert a WAV file to FLAC. Returns `Ok(false)` if `flac` ran but failed.
fn convert_to_flac(merged_wav: &Path, flac_path: &Path) -> Result<bool, Error> {
    let status = Command::new("flac")
        .args([
            "--best",
            "--silent",
            "-f",
            "-o",
            flac_path.to_str().unwrap(),
            merged_wav.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| {
            format!(
                "Failed to run `flac`: {}. Install it with: pacman -S flac",
                e
            )
        })?;

    if !status.success() {
        return Ok(false);
    }

    let flac_size = fs::metadata(flac_path)?.len();
    info!(
        "FLAC created: {:?} ({:.1} MB)",
        flac_path,
        flac_size as f64 / 1024.0 / 1024.0
    );

    Ok(true)
}

/// Split a FLAC file into `PART_SIZE` chunks. Returns human-readable part descriptions.
fn split_flac_into_parts(
    flac_path: &Path,
    output_dir: &Path,
    timestamp: &str,
) -> Result<Vec<String>, Error> {
    let mut file = File::open(flac_path)?;
    let mut buf = vec![0u8; PART_SIZE];
    let mut part_num = 0u32;
    let mut parts: Vec<String> = Vec::new();

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let part_path = output_dir.join(format!("{}-part-{}.flac", timestamp, part_num));
        File::create(&part_path)?.write_all(&buf[..n])?;
        info!(
            "Wrote {:?} ({:.1} MB)",
            part_path,
            n as f64 / 1024.0 / 1024.0
        );
        parts.push(format!(
            "`{}` ({:.1} MB)",
            part_path.file_name().unwrap().to_str().unwrap(),
            n as f64 / 1024.0 / 1024.0
        ));
        part_num += 1;
    }

    Ok(parts)
}

/// Convert the merged WAV for a session to FLAC and split into 300 MB parts if needed
#[poise::command(prefix_command, slash_command, rename = "export-audio")]
pub async fn export_audio(
    ctx: Context<'_>,
    #[description = "Session directory path (e.g. recordings/715908438760357910/2026_01_03_18_49_53)"]
    session_dir: String,
) -> Result<(), Error> {
    ctx.defer().await?;

    let session_path = PathBuf::from(&session_dir);
    let merged_wav = session_path.join("output").join("merged.wav");

    if !merged_wav.exists() {
        ctx.say("No `output/merged.wav` found. Run `/reconstruct-audio` first.")
            .await?;
        return Ok(());
    }

    let output_dir = session_path.join("output");
    let flac_path = output_dir.join("merged.flac");

    ctx.say("Converting `merged.wav` to FLAC...").await?;

    if !convert_to_flac(&merged_wav, &flac_path)? {
        ctx.say("FLAC conversion failed.").await?;
        return Ok(());
    }

    let timestamp = session_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session");

    let parts = split_flac_into_parts(&flac_path, &output_dir, timestamp)?;

    fs::remove_file(&flac_path)?;

    ctx.say(format!(
        "Done! {} part(s):\n{}",
        parts.len(),
        parts.join("\n")
    ))
    .await?;

    Ok(())
}
