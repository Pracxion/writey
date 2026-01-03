use super::storage::AudioFrame;
use tracing::info;

pub fn stereo_to_mono(stereo: &[i16]) -> Vec<i16> {
    stereo
        .chunks(2)
        .map(|chunk| {
            if chunk.len() == 2 {
                ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16
            } else {
                chunk[0]
            }
        })
        .collect()
}
