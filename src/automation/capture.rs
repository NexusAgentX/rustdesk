use super::{
    frames::{FrameError, FrameRead, FrameStamp, PixelFormat},
    sessions::{ConnectionState, Display, SessionHandle, SessionKind},
};
use chrono::{DateTime, Utc};
use hbb_common::tokio::{self, sync::Semaphore};
use image::{codecs::png::PngEncoder, imageops::FilterType, ColorType, ImageEncoder, RgbImage};
use std::{
    io::{self, Write},
    sync::{Arc, OnceLock},
};

pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    pub max_width: u32,
    pub max_height: u32,
    pub after_frame_seq: Option<u64>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            max_width: 1600,
            max_height: 1600,
            after_frame_seq: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureError {
    Frame(FrameError),
    InvalidDimensions,
    DisplayUnavailable,
    WrongSessionKind,
    ImageTooLarge,
    Encoding(String),
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub struct CapturedImage {
    pub png: Vec<u8>,
    pub stamp: FrameStamp,
    pub image_width: u32,
    pub image_height: u32,
    pub remote_rect: RemoteRect,
    pub received_at: DateTime<Utc>,
    pub age_ms: u64,
    pub is_stale: bool,
    pub is_new: Option<bool>,
    pub disconnected: bool,
    pub cursor_composited: bool,
    pub cursor_embedded: bool,
}

impl CapturedImage {
    /// This only converts geometry. The input layer must also validate binding,
    /// control authority, connection/layout generations and mapping expiry.
    pub fn remote_point(&self, x: u32, y: u32) -> Option<(i32, i32)> {
        Some((
            coordinate(
                x,
                self.image_width,
                self.remote_rect.x,
                self.remote_rect.width,
            )?,
            coordinate(
                y,
                self.image_height,
                self.remote_rect.y,
                self.remote_rect.height,
            )?,
        ))
    }
}

fn coordinate(pixel: u32, image_size: u32, origin: i32, remote_size: u32) -> Option<i32> {
    if pixel >= image_size || remote_size == 0 {
        return None;
    }
    let offset = ((u128::from(pixel) * 2 + 1) * u128::from(remote_size)
        / (u128::from(image_size) * 2))
        .min(u128::from(remote_size - 1));
    i32::try_from(i64::from(origin) + i64::try_from(offset).ok()?).ok()
}

/// Reads one actual display. Primary-display resolution, waiting and agent-bound
/// snapshot IDs belong to the higher-level session API, not the PNG worker.
pub async fn capture(
    session: &SessionHandle,
    display_id: usize,
    options: CaptureOptions,
) -> Result<Option<CapturedImage>, CaptureError> {
    if !(1..=3840).contains(&options.max_width) || !(1..=3840).contains(&options.max_height) {
        return Err(CaptureError::InvalidDimensions);
    }
    static WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = WORKERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| CaptureError::WorkerUnavailable)?;
    let snapshot = session.snapshot();
    if snapshot.kind != SessionKind::Desktop {
        return Err(CaptureError::WrongSessionKind);
    }
    let display = snapshot
        .displays
        .iter()
        .find(|d| d.id == display_id)
        .ok_or(CaptureError::DisplayUnavailable)?;
    let rect = remote_rect(display)?;
    let frame = session
        .read_frame(display_id, options.after_frame_seq)
        .map_err(|error| {
            CaptureError::Frame(if error == FrameError::NoFrame {
                snapshot.frame_error.unwrap_or(error)
            } else {
                error
            })
        })?;
    if frame.frame.stamp.connection_epoch != snapshot.connection_epoch {
        return Err(CaptureError::Frame(FrameError::StaleConnection));
    }
    if frame.frame.stamp.layout_revision != snapshot.layout_revision {
        return Err(CaptureError::Frame(FrameError::StaleLayout));
    }
    if frame.newer_than_cursor == Some(false) {
        return Ok(None);
    }
    // Keep the permit inside the worker: cancelling the caller does not stop
    // spawn_blocking, and must not let another encoder bypass the memory bound.
    let (mut result, received) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let received = frame.frame.captured_at;
        encode(frame, rect, options, MAX_PNG_BYTES).map(|result| (result, received))
    })
    .await
    .map_err(|_| CaptureError::WorkerUnavailable)??;
    let current = session
        .capture_state(result.stamp)
        .map_err(CaptureError::Frame)?;
    result.disconnected = current == ConnectionState::Disconnected;
    result.age_ms = received.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    result.is_stale = result.disconnected || result.age_ms > 2000;
    Ok(Some(result))
}

fn remote_rect(display: &Display) -> Result<RemoteRect, CaptureError> {
    if display.width <= 0 || display.height <= 0 {
        return Err(CaptureError::DisplayUnavailable);
    }
    Ok(RemoteRect {
        x: display.x,
        y: display.y,
        width: display.width as u32,
        height: display.height as u32,
    })
}

fn encode(
    read: FrameRead,
    remote_rect: RemoteRect,
    options: CaptureOptions,
    limit: usize,
) -> Result<CapturedImage, CaptureError> {
    let frame = &read.frame;
    let width = u32::try_from(frame.width).map_err(|_| CaptureError::InvalidDimensions)?;
    let height = u32::try_from(frame.height).map_err(|_| CaptureError::InvalidDimensions)?;
    let bytes = frame
        .width
        .checked_mul(frame.height)
        .and_then(|size| size.checked_mul(3))
        .ok_or(CaptureError::InvalidDimensions)?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(bytes)
        .map_err(|_| CaptureError::Frame(FrameError::Capacity))?;
    for row in frame.pixels().chunks_exact(frame.stride) {
        for pixel in row[..frame.width * 4].chunks_exact(4) {
            match frame.format {
                PixelFormat::Rgba => rgb.extend_from_slice(&pixel[..3]),
                PixelFormat::Bgra => rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]),
            }
        }
    }
    let image = RgbImage::from_raw(width, height, rgb).ok_or(CaptureError::InvalidDimensions)?;
    let (out_width, out_height) = fit(width, height, options.max_width, options.max_height);
    let image = if (out_width, out_height) == (width, height) {
        image
    } else {
        image::imageops::resize(&image, out_width, out_height, FilterType::Triangle)
    };
    let mut output = BoundedPng {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    if let Err(error) = PngEncoder::new(&mut output).write_image(
        image.as_raw(),
        out_width,
        out_height,
        ColorType::Rgb8,
    ) {
        return Err(if output.exceeded {
            CaptureError::ImageTooLarge
        } else {
            CaptureError::Encoding(error.to_string())
        });
    }
    Ok(CapturedImage {
        png: output.bytes,
        stamp: frame.stamp,
        image_width: out_width,
        image_height: out_height,
        remote_rect,
        received_at: frame.received_at,
        age_ms: 0,
        is_stale: read.is_stale(),
        is_new: read.newer_than_cursor,
        disconnected: read.disconnected,
        cursor_composited: false,
        cursor_embedded: frame.cursor_embedded,
    })
}

fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    if u64::from(max_width) * u64::from(height) <= u64::from(max_height) * u64::from(width) {
        (
            max_width,
            (u64::from(height) * u64::from(max_width) / u64::from(width)).max(1) as u32,
        )
    } else {
        (
            (u64::from(width) * u64::from(max_height) / u64::from(height)).max(1) as u32,
            max_height,
        )
    }
}

struct BoundedPng {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedPng {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "PNG size limit exceeded",
            ));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::frames::FrameCache;
    use scrap::{ImageFormat, ImageRgb};

    fn encoded(format: ImageFormat, limit: usize) -> Result<CapturedImage, CaptureError> {
        let pixels = match format {
            ImageFormat::ARGB => vec![0, 0, 255, 0, 0, 255, 0, 0, 91, 92, 93, 94],
            _ => vec![255, 0, 0, 0, 0, 255, 0, 0, 91, 92, 93, 94],
        };
        let image = ImageRgb {
            w: 2,
            h: 1,
            fmt: format,
            align: 0,
            raw: pixels,
        };
        let mut cache = FrameCache::new(128);
        cache
            .copy(
                "s",
                FrameStamp {
                    connection_epoch: 1,
                    layout_revision: 2,
                    display_id: 3,
                    sequence: 4,
                },
                &image,
                true,
            )
            .unwrap();
        encode(
            cache.read("s", 3, 1, 2, None).unwrap(),
            RemoteRect {
                x: -1920,
                y: -1080,
                width: 1920,
                height: 1080,
            },
            CaptureOptions::default(),
            limit,
        )
    }

    #[test]
    fn png_preserves_colors_ignores_padding_and_does_not_composite_cursor() {
        for format in [ImageFormat::ARGB, ImageFormat::ABGR] {
            let output = encoded(format, MAX_PNG_BYTES).unwrap();
            let image = image::load_from_memory(&output.png).unwrap().to_rgb8();
            assert_eq!(image.dimensions(), (2, 1));
            assert_eq!(image.as_raw(), &[255, 0, 0, 0, 255, 0]);
            assert!(output.cursor_embedded);
            assert!(!output.cursor_composited);
            assert_eq!(output.is_new, None);
        }
    }

    #[test]
    fn oversized_png_fails_instead_of_returning_truncated_image() {
        assert_eq!(
            encoded(ImageFormat::ARGB, 16).err(),
            Some(CaptureError::ImageTooLarge)
        );
    }

    #[test]
    fn coordinate_mapping_uses_remote_origin_and_pixel_centers() {
        let output = encoded(ImageFormat::ARGB, MAX_PNG_BYTES).unwrap();
        assert_eq!(output.remote_point(0, 0), Some((-1440, -540)));
        assert_eq!(output.remote_point(1, 0), Some((-480, -540)));
        assert_eq!(output.remote_point(2, 0), None);
        assert_eq!(output.remote_point(0, 1), None);
        assert_eq!(coordinate(0, 1, -10, 1), Some(-10));
        assert_eq!(coordinate(0, 1, i32::MAX, 2), None);
    }

    #[test]
    fn resize_fits_both_bounds_without_upscaling() {
        assert_eq!(fit(3840, 2160, 1600, 1600), (1600, 900));
        assert_eq!(fit(2160, 3840, 1600, 1600), (900, 1600));
        assert_eq!(fit(800, 600, 1600, 1600), (800, 600));
        assert_eq!(fit(10000, 1, 1, 1), (1, 1));
        assert_eq!(fit(1, 10000, 1, 1), (1, 1));
    }
}
