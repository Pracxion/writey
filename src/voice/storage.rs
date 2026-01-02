//! Sparse audio storage with tick-indexed append-only file format.
//!
//! File format per user:
//! - Header: magic bytes + version + sample_rate + channels
//! - Entries: [tick_index: u64][frame_len: u16][audio_data: [i16; frame_len]]
//!
//! Silence is implicit - missing tick indices mean no audio.

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// Magic bytes to identify our file format
const MAGIC: &[u8; 4] = b"WRTY";
/// File format version
const VERSION: u8 = 1;

#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Global tick index (each tick = 20ms)
    pub tick_index: u64,
    /// PCM samples (mono, 16-bit)
    pub samples: Vec<i16>,
}

/// Header for the sparse audio file
#[derive(Debug, Clone, Copy)]
pub struct FileHeader {
    pub version: u8,
    pub sample_rate: u32,
    pub channels: u16,
}

impl FileHeader {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            version: VERSION,
            sample_rate,
            channels,
        }
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(MAGIC)?;
        writer.write_u8(self.version)?;
        writer.write_u32::<LittleEndian>(self.sample_rate)?;
        writer.write_u16::<LittleEndian>(self.channels)?;
        Ok(())
    }

    pub fn read_from<R: Read>(reader: &mut R) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid file magic",
            ));
        }

        let version = reader.read_u8()?;
        let sample_rate = reader.read_u32::<LittleEndian>()?;
        let channels = reader.read_u16::<LittleEndian>()?;

        Ok(Self {
            version,
            sample_rate,
            channels,
        })
    }

    /// Header size in bytes
    pub const fn size() -> usize {
        4 + 1 + 4 + 2 // magic + version + sample_rate + channels
    }
}

#[derive(Debug)]
pub struct SparseAudioWriter {
    writer: BufWriter<File>,
    header: FileHeader,
    frames_written: u64,
}

impl SparseAudioWriter {
    /// Create a new sparse audio file
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;

        let mut writer = BufWriter::new(file);
        let header = FileHeader::new(sample_rate, channels);
        header.write_to(&mut writer)?;
        writer.flush()?;

        Ok(Self {
            writer,
            header,
            frames_written: 0,
        })
    }

    /// Append an audio frame
    pub fn write_frame(&mut self, frame: &AudioFrame) -> io::Result<()> {
        // Write tick index (8 bytes)
        self.writer.write_u64::<LittleEndian>(frame.tick_index)?;

        // Write frame length (2 bytes, number of samples)
        let frame_len = frame.samples.len() as u16;
        self.writer.write_u16::<LittleEndian>(frame_len)?;

        // Write samples
        for &sample in &frame.samples {
            self.writer.write_i16::<LittleEndian>(sample)?;
        }

        self.frames_written += 1;

        // Flush periodically to ensure data is written to disk
        if self.frames_written % 50 == 0 {
            self.writer.flush()?;
        }

        Ok(())
    }

    /// Flush and finalize the file
    pub fn finalize(mut self) -> io::Result<()> {
        self.writer.flush()?;
        info!("Finalized sparse audio file with {} frames", self.frames_written);
        Ok(())
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }
}

/// Reader for sparse audio files
pub struct SparseAudioReader {
    reader: BufReader<File>,
    header: FileHeader,
}

impl SparseAudioReader {
    /// Open an existing sparse audio file
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let header = FileHeader::read_from(&mut reader)?;

        Ok(Self { reader, header })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Read the next frame, returns None at EOF
    pub fn read_frame(&mut self) -> io::Result<Option<AudioFrame>> {
        // Try to read tick index
        let tick_index = match self.reader.read_u64::<LittleEndian>() {
            Ok(idx) => idx,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };

        // Read frame length
        let frame_len = self.reader.read_u16::<LittleEndian>()? as usize;

        // Read samples
        let mut samples = Vec::with_capacity(frame_len);
        for _ in 0..frame_len {
            samples.push(self.reader.read_i16::<LittleEndian>()?);
        }

        Ok(Some(AudioFrame { tick_index, samples }))
    }

    /// Read all frames into memory (use for short files only)
    pub fn read_all_frames(&mut self) -> io::Result<Vec<AudioFrame>> {
        let mut frames = Vec::new();
        while let Some(frame) = self.read_frame()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Stream frames with O(1) memory - use iterator
    pub fn into_iter(self) -> SparseAudioIterator {
        SparseAudioIterator { reader: self }
    }
}

/// Iterator over sparse audio frames (O(1) memory)
pub struct SparseAudioIterator {
    reader: SparseAudioReader,
}

impl Iterator for SparseAudioIterator {
    type Item = io::Result<AudioFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.read_frame() {
            Ok(Some(frame)) => Some(Ok(frame)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[derive(Debug)]
pub struct SessionStorage {
    /// Base directory for this session
    session_dir: PathBuf,
    /// Writers per user (user_id -> writer)
    writers: HashMap<u64, SparseAudioWriter>,
    /// Sample rate
    sample_rate: u32,
    /// Channels (1 for mono)
    channels: u16,
}

impl SessionStorage {
    /// Create a new session storage
    pub fn new(session_dir: PathBuf, sample_rate: u32, channels: u16) -> io::Result<Self> {
        std::fs::create_dir_all(&session_dir)?;
        info!("Created session storage at {:?}", session_dir);

        Ok(Self {
            session_dir,
            writers: HashMap::new(),
            sample_rate,
            channels,
        })
    }

    /// Get or create a writer for a user
    pub fn get_or_create_writer(&mut self, user_id: u64) -> io::Result<&mut SparseAudioWriter> {
        if !self.writers.contains_key(&user_id) {
            let path = self.session_dir.join(format!("user_{}.wrty", user_id));
            let writer = SparseAudioWriter::create(&path, self.sample_rate, self.channels)?;
            self.writers.insert(user_id, writer);
            info!("Created audio file for user {}", user_id);
        }
        Ok(self.writers.get_mut(&user_id).unwrap())
    }

    /// Write a frame for a user
    pub fn write_frame(&mut self, user_id: u64, frame: &AudioFrame) -> io::Result<()> {
        let writer = self.get_or_create_writer(user_id)?;
        writer.write_frame(frame)
    }

    /// Finalize all writers
    pub fn finalize(self) -> io::Result<Vec<(u64, PathBuf)>> {
        let mut files = Vec::new();
        for (user_id, writer) in self.writers {
            let path = self.session_dir.join(format!("user_{}.wrty", user_id));
            writer.finalize()?;
            files.push((user_id, path));
        }
        Ok(files)
    }

    /// Get the session directory
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// List all user files in the session
    pub fn list_user_files(&self) -> io::Result<Vec<(u64, PathBuf)>> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "wrty") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Some(user_id_str) = stem.strip_prefix("user_") {
                        if let Ok(user_id) = user_id_str.parse::<u64>() {
                            files.push((user_id, path));
                        }
                    }
                }
            }
        }
        Ok(files)
    }
}

/// Rebuild continuous PCM from sparse frames (with silence gaps filled)
pub fn rebuild_continuous_pcm(
    frames: &[AudioFrame],
    samples_per_tick: usize,
) -> Vec<i16> {
    if frames.is_empty() {
        return Vec::new();
    }

    let first_tick = frames.first().map(|f| f.tick_index).unwrap_or(0);
    let last_tick = frames.last().map(|f| f.tick_index).unwrap_or(0);
    let total_ticks = (last_tick - first_tick + 1) as usize;

    let mut pcm = vec![0i16; total_ticks * samples_per_tick];

    for frame in frames {
        let relative_tick = (frame.tick_index - first_tick) as usize;
        let start_idx = relative_tick * samples_per_tick;

        // Copy samples, handling potential size mismatches
        let copy_len = frame.samples.len().min(samples_per_tick);
        pcm[start_idx..start_idx + copy_len].copy_from_slice(&frame.samples[..copy_len]);
    }

    pcm
}

/// Stream-based continuous PCM rebuild with O(1) memory per chunk
pub struct StreamingPcmRebuilder {
    samples_per_tick: usize,
    current_tick: u64,
    buffer: Vec<i16>,
}

impl StreamingPcmRebuilder {
    pub fn new(samples_per_tick: usize, start_tick: u64) -> Self {
        Self {
            samples_per_tick,
            current_tick: start_tick,
            buffer: Vec::with_capacity(samples_per_tick),
        }
    }

    /// Process a frame, returns PCM chunks to write (including silence)
    pub fn process_frame(&mut self, frame: &AudioFrame) -> Vec<i16> {
        let mut output = Vec::new();

        // Fill silence for missing ticks
        while self.current_tick < frame.tick_index {
            output.extend(std::iter::repeat(0i16).take(self.samples_per_tick));
            self.current_tick += 1;
        }

        // Add the actual frame
        output.extend_from_slice(&frame.samples);
        self.current_tick = frame.tick_index + 1;

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sparse_audio_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wrty");

        // Write frames
        {
            let mut writer = SparseAudioWriter::create(&path, 48000, 1).unwrap();

            writer
                .write_frame(&AudioFrame {
                    tick_index: 0,
                    samples: vec![100, 200, 300],
                })
                .unwrap();

            writer
                .write_frame(&AudioFrame {
                    tick_index: 5, // Gap of 4 ticks (implicit silence)
                    samples: vec![400, 500],
                })
                .unwrap();

            writer.finalize().unwrap();
        }

        // Read frames back
        {
            let mut reader = SparseAudioReader::open(&path).unwrap();
            assert_eq!(reader.header().sample_rate, 48000);
            assert_eq!(reader.header().channels, 1);

            let frames = reader.read_all_frames().unwrap();
            assert_eq!(frames.len(), 2);

            assert_eq!(frames[0].tick_index, 0);
            assert_eq!(frames[0].samples, vec![100, 200, 300]);

            assert_eq!(frames[1].tick_index, 5);
            assert_eq!(frames[1].samples, vec![400, 500]);
        }
    }
}

