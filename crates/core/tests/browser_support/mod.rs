//! Helpers shared by the browser end-to-end tests.

use std::sync::Arc;
use std::time::Duration;

/// The page's text once `want` holds, or after 10 s: input reaches a page asynchronously,
/// so a snapshot right after an action may come before the page's handlers ran.
pub async fn page_once(
    chat: &Arc<themis_core::browser::ChatBrowser>,
    want: impl Fn(&str) -> bool,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let text = chat.snapshot(None, false).await.unwrap().snapshot;
        if want(&text) || std::time::Instant::now() > deadline {
            return text;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub fn frame_colour(data: &str) -> ([u8; 3], usize, usize) {
    use base64::Engine;
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let (rgb, width, height) = themis_core::browser::media::decode_jpeg(&jpeg).unwrap();
    let mut sum = [0u64; 3];
    for pixel in rgb.as_chunks::<3>().0 {
        for (total, value) in sum.iter_mut().zip(pixel) {
            *total += u64::from(*value);
        }
    }
    let count = (width * height).max(1) as u64;
    (sum.map(|total| (total / count) as u8), width, height)
}

/// Waits up to `limit` for a frame of the stream that `matches`; returns how many came.
pub async fn frame_where(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
    limit: Duration,
    matches: impl Fn([u8; 3], usize, usize) -> bool,
) -> Result<usize, usize> {
    let mut seen = 0;
    let found = tokio::time::timeout(limit, async {
        loop {
            match updates.recv().await {
                Ok(themis_core::browser::BrowserUpdate::Frame { data, .. }) => {
                    seen += 1;
                    let (colour, width, height) = frame_colour(&data);
                    if matches(colour, width, height) {
                        return;
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await;
    found.map(|()| seen).map_err(|_| seen)
}
