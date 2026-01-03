use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

const TICK_FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const SSRC_MAP_FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const CHUNK_DURATION: Duration = Duration::from_secs(10 * 60); // 10 minutes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub tick_index: u64,
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub enum StorageMessage {
    Frame { ssrc: u32, frame: AudioFrame },
    SsrcMap(HashMap<u32, u64>),
    Flush,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    tx: mpsc::UnboundedSender<StorageMessage>,
}

impl StorageHandle {
    pub fn buffer_frame(&self, ssrc: u32, frame: AudioFrame) {
        let _ = self.tx.send(StorageMessage::Frame { ssrc, frame });
    }

    pub fn update_ssrc_map(&self, ssrc_map: HashMap<u32, u64>) {
        let _ = self.tx.send(StorageMessage::SsrcMap(ssrc_map));
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(StorageMessage::Shutdown);
    }
}

struct SsrcChunkState {
    current_chunk: u32,
    chunk_start: Instant,
}

pub struct StorageWriter {
    session_dir: PathBuf,
    users_dir: PathBuf,
    /// Buffers frames by ssrc
    buffers: HashMap<u32, Vec<AudioFrame>>,
    /// Maps ssrc -> user_id (for reference only)
    ssrc_map: HashMap<u32, u64>,
    /// Tracks chunk state per ssrc
    ssrc_chunks: HashMap<u32, SsrcChunkState>,
    session_start: Instant,
    last_tick_flush: Instant,
    last_ssrc_map_flush: Instant,
    rx: mpsc::UnboundedReceiver<StorageMessage>,
}

impl StorageWriter {
    pub fn new(session_dir: PathBuf) -> io::Result<(StorageHandle, Self)> {
        std::fs::create_dir_all(&session_dir)?;
        let users_dir = session_dir.join("users");
        std::fs::create_dir_all(&users_dir)?;
        info!("Created session storage at {:?}", session_dir);

        let (tx, rx) = mpsc::unbounded_channel();

        let handle = StorageHandle { tx };
        let now = Instant::now();
        let writer = Self {
            session_dir,
            users_dir,
            buffers: HashMap::new(),
            ssrc_map: HashMap::new(),
            ssrc_chunks: HashMap::new(),
            session_start: now,
            last_tick_flush: now,
            last_ssrc_map_flush: now,
            rx,
        };

        Ok((handle, writer))
    }

    pub async fn run(mut self) {
        info!("Storage writer task started");

        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.rx.recv()).await {
                Ok(Some(msg)) => match msg {
                    StorageMessage::Frame { ssrc, frame } => {
                        self.buffers.entry(ssrc).or_default().push(frame);
                    }
                    StorageMessage::SsrcMap(map) => {
                        self.ssrc_map = map;
                    }
                    StorageMessage::Flush => {
                        if let Err(e) = self.flush_all() {
                            error!("Failed to flush: {}", e);
                        }
                    }
                    StorageMessage::Shutdown => {
                        info!("Storage writer shutting down");
                        if let Err(e) = self.flush_all() {
                            error!("Failed to flush on shutdown: {}", e);
                        }
                        break;
                    }
                },
                Ok(None) => {
                    info!("Storage channel closed, flushing and exiting");
                    let _ = self.flush_all();
                    break;
                }
                Err(_) => {}
            }

            if let Err(e) = self.try_flush() {
                warn!("Periodic flush failed: {}", e);
            }
        }

        info!("Storage writer task ended");
    }

    fn try_flush(&mut self) -> io::Result<()> {
        if self.last_tick_flush.elapsed() >= TICK_FLUSH_INTERVAL {
            self.flush_ticks()?;
        }
        if self.last_ssrc_map_flush.elapsed() >= SSRC_MAP_FLUSH_INTERVAL {
            self.flush_ssrc_map()?;
        }
        Ok(())
    }

    fn flush_all(&mut self) -> io::Result<()> {
        self.flush_ticks()?;
        self.flush_ssrc_map()?;
        Ok(())
    }

    fn get_chunk_for_ssrc(&mut self, ssrc: u32) -> u32 {
        let entry = self
            .ssrc_chunks
            .entry(ssrc)
            .or_insert_with(|| SsrcChunkState {
                current_chunk: 0,
                chunk_start: Instant::now(),
            });

        if entry.chunk_start.elapsed() >= CHUNK_DURATION {
            entry.current_chunk += 1;
            entry.chunk_start = Instant::now();
        }

        entry.current_chunk
    }

    fn flush_ticks(&mut self) -> io::Result<()> {
        let total_frames: usize = self.buffers.values().map(|v| v.len()).sum();
        if total_frames == 0 {
            self.last_tick_flush = Instant::now();
            return Ok(());
        }

        info!("Flushing {} buffered frames to disk", total_frames);

        // Collect ssrcs and their chunks first
        let ssrcs: Vec<u32> = self.buffers.keys().cloned().collect();
        let mut ssrc_chunk_map: HashMap<u32, u32> = HashMap::new();
        for ssrc in &ssrcs {
            ssrc_chunk_map.insert(*ssrc, self.get_chunk_for_ssrc(*ssrc));
        }

        let frames_to_flush: Vec<(u32, Vec<AudioFrame>)> = self.buffers.drain().collect();
        let users_dir = self.users_dir.clone();

        tokio::task::spawn_blocking(move || {
            for (ssrc, frames) in frames_to_flush {
                let chunk_num = ssrc_chunk_map.get(&ssrc).copied().unwrap_or(0);
                let ssrc_dir = users_dir.join(ssrc.to_string());

                if let Err(e) = std::fs::create_dir_all(&ssrc_dir) {
                    error!("Failed to create ssrc dir {:?}: {}", ssrc_dir, e);
                    continue;
                }

                let chunk_path = ssrc_dir.join(format!("chunk-{}.log", chunk_num));

                let file = match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&chunk_path)
                {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Failed to open chunk file {:?}: {}", chunk_path, e);
                        continue;
                    }
                };

                let mut writer = BufWriter::new(file);

                for frame in frames {
                    // Format: tick_index sample1,sample2,sample3,...
                    let samples_str: String = frame
                        .samples
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(",");

                    if let Err(e) = writeln!(writer, "{} {}", frame.tick_index, samples_str) {
                        error!("Failed to write frame: {}", e);
                    }
                }

                if let Err(e) = writer.flush() {
                    error!("Failed to flush writer: {}", e);
                }
            }
            Ok::<(), io::Error>(())
        });

        self.last_tick_flush = Instant::now();
        Ok(())
    }

    fn flush_ssrc_map(&mut self) -> io::Result<()> {
        if self.ssrc_map.is_empty() {
            self.last_ssrc_map_flush = Instant::now();
            return Ok(());
        }

        info!("Flushing ssrc_map with {} entries", self.ssrc_map.len());

        let ssrc_map = self.ssrc_map.clone();
        self.ssrc_map.clear();
        let path = self.session_dir.join("ssrc_map");

        tokio::task::spawn_blocking(move || {
            let file = File::create(&path)?;
            let writer = BufWriter::new(file);
            serde_json::to_writer_pretty(writer, &ssrc_map)?;
            Ok::<(), io::Error>(())
        });

        self.last_ssrc_map_flush = Instant::now();
        Ok(())
    }
}
