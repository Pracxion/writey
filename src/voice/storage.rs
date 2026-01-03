use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{info, warn, error};
use poise::futures_util::TryFutureExt;

const TICK_FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const SSRC_MAP_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub tick_index: u64,
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub enum StorageMessage {
    Frame { ssrc: u64, frame: AudioFrame },
    SsrcMap(HashMap<u32, u64>),
    Flush,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    tx: mpsc::UnboundedSender<StorageMessage>,
}

impl StorageHandle {
    pub fn buffer_frame(&self, ssrc: u64, frame: AudioFrame) {
        let _ = self.tx.send(StorageMessage::Frame { ssrc, frame });
    }

    pub fn update_ssrc_map(&self, ssrc_map: HashMap<u32, u64>) {
        let _ = self.tx.send(StorageMessage::SsrcMap(ssrc_map));
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(StorageMessage::Shutdown);
    }
}

pub struct StorageWriter {
    session_dir: PathBuf,
    buffers: HashMap<u64, Vec<AudioFrame>>,
    ssrc_map: HashMap<u32, u64>,
    last_tick_flush: Instant,
    last_ssrc_map_flush: Instant,
    rx: mpsc::UnboundedReceiver<StorageMessage>,
}

impl StorageWriter {
    pub fn new(session_dir: PathBuf) -> io::Result<(StorageHandle, Self)> {
        std::fs::create_dir_all(&session_dir)?;
        info!("Created session storage at {:?}", session_dir);

        let (tx, rx) = mpsc::unbounded_channel();

        let handle = StorageHandle { tx };
        let writer = Self {
            session_dir,
            buffers: HashMap::new(),
            ssrc_map: HashMap::new(),
            last_tick_flush: Instant::now(),
            last_ssrc_map_flush: Instant::now(),
            rx,
        };

        Ok((handle, writer))
    }

    pub async fn run(mut self) {
        info!("Storage writer task started");

        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.rx.recv()).await {
                Ok(Some(msg)) => {
                    match msg {
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
                    }
                }
                Ok(None) => {
                    info!("Storage channel closed, flushing and exiting");
                    let _ = self.flush_all();
                    break;
                }
                Err(_) => {
                }
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

    fn flush_ticks(&mut self) -> io::Result<()> {
        let total_frames: usize = self.buffers.values().map(|v| v.len()).sum();
        if total_frames == 0 {
            self.last_tick_flush = Instant::now();
            return Ok(());
        }
    
        info!("Flushing {} buffered frames to disk", total_frames);
    
        let frames_to_flush: Vec<(u64, Vec<AudioFrame>)> = self.buffers.drain().collect();
        let session_dir = self.session_dir.clone();
    
        let session_dir_clone = session_dir.clone();
        tokio::task::spawn_blocking(move || {
            for (ssrc, frames) in frames_to_flush {
                let path = session_dir_clone.join(format!("{}.json", ssrc));
    
                let mut all_frames: Vec<AudioFrame> = if path.exists() {
                    let file = File::open(&path)?;
                    serde_json::from_reader(file).unwrap_or_default()
                } else {
                    Vec::new()
                };
    
                all_frames.extend(frames);
    
                let file = File::create(&path)?;
                let writer = BufWriter::new(file);
                serde_json::to_writer(writer, &all_frames)?;
            }
            Ok::<(), io::Error>(())
        })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Task join error: {}", e)));
    
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
        let path = self.session_dir.join("ssrc_map.json");
        
        tokio::task::spawn_blocking(move || {
            let file = File::create(&path)?;
            let writer = BufWriter::new(file);
            serde_json::to_writer_pretty(writer, &ssrc_map)?;
            Ok::<(), io::Error>(())
        })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Task join error: {}", e)));
    
        self.last_ssrc_map_flush = Instant::now();
        Ok(())
    }
}
