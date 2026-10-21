//! Screenshots and recordings of the built-in browser.
//!
//! A recording keeps the screencast's JPEG frames (sampled to at most
//! [`RECORD_FPS`]) and encodes them to H.264 in an MP4 when it stops, with the
//! open-source OpenH264 encoder built into the app, so no ffmpeg is needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Frames kept per second of a recording.
pub const RECORD_FPS: u32 = 10;
/// Default and longest recording.
pub const RECORD_DEFAULT: Duration = Duration::from_secs(30);
pub const RECORD_MAX: Duration = Duration::from_secs(120);
/// Memory a recording's frames may use before it stops collecting.
pub const RECORD_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Where screenshots and recordings are saved (the desktop keeps them with
/// the project's attachments, so replies can show them).
pub trait MediaSink: Send + Sync {
    /// Saves `bytes` as `name` and returns the file's absolute path.
    fn save(&self, name: &str, bytes: &[u8]) -> Result<String, String>;
}

/// A file name like `screenshot-1652-03.png` from the local time.
pub fn media_name(kind: &str, extension: &str) -> String {
    let (hours, minutes, seconds) = local_clock();
    format!("{kind}-{hours:02}{minutes:02}-{seconds:02}.{extension}")
}

/// Local hours, minutes and seconds (UTC where the zone is unknown).
fn local_clock() -> (i64, i64, i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    #[cfg(unix)]
    {
        let time = now as libc::time_t;
        // SAFETY: localtime_r writes only into the struct we pass.
        let mut parts: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&time, &mut parts) }.is_null() {
            return (
                i64::from(parts.tm_hour),
                i64::from(parts.tm_min),
                i64::from(parts.tm_sec),
            );
        }
    }
    ((now / 3600) % 24, (now / 60) % 60, now % 60)
}

/// Who started a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recorder {
    Agent,
    User,
}

/// One screencast frame of a recording, `at` after it started.
#[derive(Debug, Clone)]
pub struct Frame {
    pub at: Duration,
    pub jpeg: Vec<u8>,
}

/// A recording in progress.
#[derive(Debug)]
pub struct Recording {
    pub by: Recorder,
    pub started: std::time::Instant,
    pub max: Duration,
    pub frames: Vec<Frame>,
    bytes: usize,
}

impl Recording {
    pub fn new(by: Recorder, max: Duration) -> Self {
        Self {
            by,
            started: std::time::Instant::now(),
            max: max.min(RECORD_MAX),
            frames: Vec::new(),
            bytes: 0,
        }
    }

    /// Keeps a frame unless it comes too soon after the last one, the time is
    /// up or memory is spent. Returns `false` once the recording is full.
    pub fn push(&mut self, jpeg: Vec<u8>) -> bool {
        let at = self.started.elapsed();
        if at > self.max || self.bytes + jpeg.len() > RECORD_MAX_BYTES {
            return false;
        }
        let spacing = Duration::from_millis(u64::from(1000 / RECORD_FPS));
        if self
            .frames
            .last()
            .is_some_and(|last| at < last.at + spacing)
        {
            return true;
        }
        self.bytes += jpeg.len();
        self.frames.push(Frame { at, jpeg });
        true
    }

    /// Whether the time limit has passed.
    pub fn is_over(&self) -> bool {
        self.started.elapsed() > self.max
    }

    /// Length so far (capped at the limit).
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed().min(self.max)
    }
}

/// The frame to show at each tick of a constant frame rate: the latest one
/// taken at or before the tick. Recording sampling is uneven (pages repaint
/// when they change), video playback is not.
pub fn frame_schedule(frames: &[Frame], fps: u32, until: Duration) -> Vec<usize> {
    if frames.is_empty() || fps == 0 {
        return Vec::new();
    }
    let step = Duration::from_secs(1) / fps;
    let ticks = (until.as_millis() / step.as_millis().max(1)).max(1) as usize;
    let mut schedule = Vec::with_capacity(ticks);
    let mut current = 0;
    for tick in 0..ticks {
        let at = step * tick as u32;
        while current + 1 < frames.len() && frames[current + 1].at <= at {
            current += 1;
        }
        schedule.push(current);
    }
    schedule
}

/// Encodes frames to an H.264 MP4 at [`RECORD_FPS`]. Frames are first shrunk
/// by `shrink` (the view's device scale, so a Retina recording is encoded at
/// CSS size: a quarter of the pixels). The size is that of the first frame
/// (rounded down to even numbers); other sizes are cropped or padded. It gives
/// up between frames once `cancelled` is set, as when the run is stopped.
pub fn encode_mp4(
    frames: &[Frame],
    length: Duration,
    shrink: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, String> {
    use openh264::encoder::{
        BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, QpRange, RateControlMode,
        UsageType,
    };
    use openh264::formats::{RgbSliceU8, YUVBuffer};

    let stopped = || {
        cancelled
            .load(Ordering::Relaxed)
            .then_some(Err("the recording was cancelled".to_owned()))
    };
    if let Some(error) = stopped() {
        return error;
    }
    let first = frames.first().ok_or("the recording has no frames")?;
    let shrink = shrink.max(1);
    let (_, width, height) = decode_jpeg(&first.jpeg)?;
    let (width, height) = ((width / shrink) & !1, (height / shrink) & !1);
    if width < 16 || height < 16 {
        return Err("the recorded page is too small".into());
    }
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(2_500_000))
        .max_frame_rate(FrameRate::from_hz(RECORD_FPS as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .skip_frames(false)
        // Constant quality: every frame kept, and none of OpenH264's warnings
        // about settings screen content does not support.
        .rate_control_mode(RateControlMode::Off)
        .qp(QpRange::new(18, 32))
        .adaptive_quantization(false)
        .background_detection(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(RECORD_FPS * 2));
    let mut encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
        .map_err(|e| format!("could not start the video encoder: {e}"))?;
    let schedule = frame_schedule(frames, RECORD_FPS, length.max(Duration::from_millis(100)));
    let mut samples: Vec<(Vec<u8>, bool)> = Vec::with_capacity(schedule.len());
    let (mut sps, mut pps) = (None, None);
    let mut decoded: Option<(usize, Vec<u8>)> = None;
    for index in schedule {
        if let Some(error) = stopped() {
            return error;
        }
        if decoded.as_ref().is_none_or(|(at, _)| *at != index) {
            let (rgb, w, h) = decode_jpeg(&frames[index].jpeg)?;
            let (rgb, w, h) = reduce(&rgb, w, h, shrink);
            decoded = Some((index, fit(&rgb, w, h, width, height)));
        }
        let rgb = &decoded.as_ref().expect("decoded above").1;
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(rgb, (width, height)));
        let stream = encoder
            .encode(&yuv)
            .map_err(|e| format!("could not encode the recording: {e}"))?;
        let mut sample = Vec::new();
        let mut sync = false;
        for layer in 0..stream.num_layers() {
            let Some(layer) = stream.layer(layer) else {
                continue;
            };
            for nal in 0..layer.nal_count() {
                let Some(unit) = layer.nal_unit(nal) else {
                    continue;
                };
                let unit = strip_start_code(unit);
                match unit.first().map(|b| b & 0x1f) {
                    Some(7) => sps = Some(unit.to_vec()),
                    Some(8) => pps = Some(unit.to_vec()),
                    Some(kind) => {
                        sync |= kind == 5;
                        sample.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                        sample.extend_from_slice(unit);
                    }
                    None => {}
                }
            }
        }
        if !sample.is_empty() {
            samples.push((sample, sync));
        }
    }
    let (Some(sps), Some(pps)) = (sps, pps) else {
        return Err("the encoder produced no video".into());
    };
    let mut out = std::io::Cursor::new(Vec::new());
    let timescale = 1000;
    let mut writer = mp4::Mp4Writer::write_start(
        &mut out,
        &mp4::Mp4Config {
            major_brand: str::parse("isom").expect("valid brand"),
            minor_version: 512,
            compatible_brands: ["isom", "iso2", "avc1", "mp41"]
                .iter()
                .map(|brand| str::parse(brand).expect("valid brand"))
                .collect(),
            timescale,
        },
    )
    .map_err(|e| e.to_string())?;
    writer
        .add_track(&mp4::TrackConfig {
            track_type: mp4::TrackType::Video,
            timescale,
            language: "und".into(),
            media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                width: u16::try_from(width).map_err(|_| "the page is too wide to record")?,
                height: u16::try_from(height).map_err(|_| "the page is too tall to record")?,
                seq_param_set: sps,
                pic_param_set: pps,
            }),
        })
        .map_err(|e| e.to_string())?;
    let duration = timescale / RECORD_FPS;
    for (index, (bytes, sync)) in samples.into_iter().enumerate() {
        writer
            .write_sample(
                1,
                &mp4::Mp4Sample {
                    start_time: index as u64 * u64::from(duration),
                    duration,
                    rendering_offset: 0,
                    is_sync: sync || index == 0,
                    bytes: bytes::Bytes::from(bytes),
                },
            )
            .map_err(|e| e.to_string())?;
    }
    writer.write_end().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out.into_inner())
}

fn strip_start_code(unit: &[u8]) -> &[u8] {
    unit.strip_prefix(&[0, 0, 0, 1])
        .or_else(|| unit.strip_prefix(&[0, 0, 1]))
        .unwrap_or(unit)
}

/// RGB pixels and size of a JPEG.
pub fn decode_jpeg(jpeg: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(jpeg));
    let pixels = decoder
        .decode()
        .map_err(|e| format!("a recorded frame could not be read: {e:?}"))?;
    let info = decoder.info().ok_or("a recorded frame has no size")?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    if pixels.len() != width * height * 3 {
        return Err("a recorded frame has an unexpected color format".into());
    }
    Ok((pixels, width, height))
}

/// `rgb` (`w`×`h`) shrunk by a whole `factor`, each pixel the average of its block.
fn reduce(rgb: &[u8], w: usize, h: usize, factor: usize) -> (Vec<u8>, usize, usize) {
    if factor <= 1 {
        return (rgb.to_vec(), w, h);
    }
    let (out_w, out_h) = (w / factor, h / factor);
    let area = (factor * factor) as u32;
    let mut out = Vec::with_capacity(out_w * out_h * 3);
    for y in 0..out_h {
        for x in 0..out_w {
            let mut sum = [0u32; 3];
            for dy in 0..factor {
                let row = (y * factor + dy) * w;
                for dx in 0..factor {
                    let at = (row + x * factor + dx) * 3;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(rgb[at + channel]);
                    }
                }
            }
            out.extend(sum.map(|total| (total / area) as u8));
        }
    }
    (out, out_w, out_h)
}

/// `rgb` (`w`×`h`) cropped or padded with white to `width`×`height`.
fn fit(rgb: &[u8], w: usize, h: usize, width: usize, height: usize) -> Vec<u8> {
    if w == width && h == height {
        return rgb.to_vec();
    }
    let mut out = vec![255; width * height * 3];
    for row in 0..height.min(h) {
        let copy = width.min(w) * 3;
        out[row * width * 3..row * width * 3 + copy]
            .copy_from_slice(&rgb[row * w * 3..row * w * 3 + copy]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(at_ms: u64) -> Frame {
        Frame {
            at: Duration::from_millis(at_ms),
            jpeg: Vec::new(),
        }
    }

    #[test]
    fn a_cancelled_encode_stops_without_a_video() {
        let frames = [frame(0), frame(100)];
        assert_eq!(
            encode_mp4(&frames, Duration::from_secs(1), 1, &AtomicBool::new(true)),
            Err("the recording was cancelled".to_owned())
        );
    }

    #[test]
    fn uneven_frames_become_a_steady_schedule() {
        let frames = [frame(0), frame(250), frame(260), frame(900)];
        assert_eq!(
            frame_schedule(&frames, 10, Duration::from_millis(1000)),
            [0, 0, 0, 2, 2, 2, 2, 2, 2, 3]
        );
        assert!(frame_schedule(&[], 10, Duration::from_secs(1)).is_empty());
        assert_eq!(frame_schedule(&frames, 10, Duration::ZERO), [0]);
    }

    #[test]
    fn a_recording_samples_frames_and_stops_at_its_limit() {
        let mut recording = Recording::new(Recorder::Agent, Duration::from_secs(1000));
        assert_eq!(recording.max, RECORD_MAX);
        assert!(recording.push(vec![1; 10]));
        // Too soon after the last frame: skipped, still recording.
        assert!(recording.push(vec![2; 10]));
        assert_eq!(recording.frames.len(), 1);
        let mut over = Recording::new(Recorder::User, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        assert!(!over.push(vec![1]));
        assert!(over.is_over());
    }

    #[test]
    fn fit_crops_and_pads_to_the_video_size() {
        let rgb: Vec<u8> = (0..2 * 2 * 3).map(|n| n as u8).collect();
        assert_eq!(fit(&rgb, 2, 2, 2, 2), rgb);
        let padded = fit(&rgb, 2, 2, 3, 3);
        assert_eq!(padded.len(), 27);
        assert_eq!(&padded[..6], &rgb[..6]);
        assert_eq!(&padded[6..9], &[255, 255, 255]);
        assert_eq!(fit(&rgb, 2, 2, 1, 1), vec![0, 1, 2]);
    }

    #[test]
    fn reduce_averages_blocks_into_a_smaller_image() {
        // 4×2 pixels: a black and a white 2×2 block, then halved to 2×1.
        let mut rgb = vec![0u8; 4 * 2 * 3];
        for row in 0..2 {
            for col in 2..4 {
                rgb[(row * 4 + col) * 3..(row * 4 + col) * 3 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        rgb[0] = 100; // one red value in the black block
        let (out, w, h) = reduce(&rgb, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![25, 0, 0, 255, 255, 255]);
        assert_eq!(reduce(&rgb, 4, 2, 1), (rgb.clone(), 4, 2));
    }

    #[test]
    fn start_codes_are_stripped() {
        assert_eq!(strip_start_code(&[0, 0, 0, 1, 0x67, 1]), &[0x67, 1]);
        assert_eq!(strip_start_code(&[0, 0, 1, 0x68]), &[0x68]);
        assert_eq!(strip_start_code(&[0x65]), &[0x65]);
    }
}
