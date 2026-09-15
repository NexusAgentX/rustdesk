use chrono::{DateTime, Utc};
use scrap::{ImageFormat, ImageRgb};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_CACHE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba,
    Bgra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    NoFrame,
    StaleConnection,
    StaleLayout,
    InvalidPixels,
    UnsupportedFormat,
    Capacity,
    CaptureDisabled,
    TextureOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStamp {
    pub connection_epoch: u64,
    pub layout_revision: u64,
    pub display_id: usize,
    pub sequence: u64,
}

#[derive(Debug)]
pub struct Frame {
    pub stamp: FrameStamp,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: PixelFormat,
    pub cursor_embedded: bool,
    pub captured_at: Instant,
    pub received_at: DateTime<Utc>,
    pixels: Vec<u8>,
    budget: Arc<AtomicUsize>,
}

impl Frame {
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.pixels.len(), Ordering::AcqRel);
    }
}

pub struct FrameRead {
    pub frame: Arc<Frame>,
    /// Freshness is relative to the caller's cursor, not an arbitrary time threshold.
    pub newer_than_cursor: Option<bool>,
    pub disconnected: bool,
}

impl FrameRead {
    pub fn is_stale(&self) -> bool {
        self.disconnected || self.frame.captured_at.elapsed().as_millis() > 2000
    }
}

struct CachedFrame {
    frame: Arc<Frame>,
    last_read: Instant,
}

pub(crate) struct FrameCache {
    entries: HashMap<(String, usize), CachedFrame>,
    used: Arc<AtomicUsize>,
    limit: usize,
}

impl Default for FrameCache {
    fn default() -> Self {
        Self::new(MAX_CACHE_BYTES)
    }
}

impl FrameCache {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            used: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    pub(crate) fn copy(
        &mut self,
        session: &str,
        stamp: FrameStamp,
        image: &ImageRgb,
        cursor_embedded: bool,
    ) -> Result<Arc<Frame>, FrameError> {
        let (format, stride) = validate(image)?;
        let bytes = image.raw.len();
        if bytes > MAX_FRAME_BYTES || bytes > self.limit {
            return Err(FrameError::Capacity);
        }
        let last_read = self
            .entries
            .remove(&(session.to_owned(), stamp.display_id))
            .map(|entry| entry.last_read)
            .unwrap_or_else(Instant::now);
        // Outstanding readers remain charged even after their cached entry is evicted.
        while self.used.load(Ordering::Acquire) > self.limit - bytes {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_read)
                .map(|(key, _)| key.clone());
            match victim {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => return Err(FrameError::Capacity),
            }
        }
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(bytes)
            .map_err(|_| FrameError::Capacity)?;
        pixels.extend_from_slice(&image.raw);
        self.used.fetch_add(bytes, Ordering::AcqRel);
        let frame = Arc::new(Frame {
            stamp,
            width: image.w,
            height: image.h,
            stride,
            format,
            cursor_embedded,
            captured_at: Instant::now(),
            received_at: Utc::now(),
            pixels,
            budget: self.used.clone(),
        });
        self.entries.insert(
            (session.to_owned(), stamp.display_id),
            CachedFrame {
                frame: frame.clone(),
                last_read,
            },
        );
        Ok(frame)
    }

    pub(crate) fn read(
        &mut self,
        session: &str,
        display: usize,
        epoch: u64,
        layout: u64,
        after: Option<u64>,
    ) -> Result<FrameRead, FrameError> {
        let entry = self
            .entries
            .get_mut(&(session.to_owned(), display))
            .ok_or(FrameError::NoFrame)?;
        if entry.frame.stamp.connection_epoch != epoch {
            return Err(FrameError::StaleConnection);
        }
        if entry.frame.stamp.layout_revision != layout {
            return Err(FrameError::StaleLayout);
        }
        entry.last_read = Instant::now();
        Ok(FrameRead {
            newer_than_cursor: after.map(|seq| entry.frame.stamp.sequence > seq),
            disconnected: false,
            frame: entry.frame.clone(),
        })
    }

    pub(crate) fn clear(&mut self, session: &str) {
        self.entries.retain(|(id, _), _| id != session);
    }
}

pub(crate) fn validate(image: &ImageRgb) -> Result<(PixelFormat, usize), FrameError> {
    let format = match image.fmt {
        ImageFormat::ABGR => PixelFormat::Rgba,
        ImageFormat::ARGB => PixelFormat::Bgra,
        ImageFormat::Raw => return Err(FrameError::UnsupportedFormat),
    };
    let row = image.w.checked_mul(4).ok_or(FrameError::InvalidPixels)?;
    if row == 0 || image.h == 0 || image.raw.len() % image.h != 0 {
        return Err(FrameError::InvalidPixels);
    }
    let stride = image.raw.len() / image.h;
    if stride < row {
        return Err(FrameError::InvalidPixels);
    }
    Ok((format, stride))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> ImageRgb {
        ImageRgb {
            raw: vec![1, 2, 3, 255, 0, 0, 0, 0],
            w: 1,
            h: 1,
            fmt: ImageFormat::ARGB,
            align: 8,
        }
    }

    fn stamp(display_id: usize) -> FrameStamp {
        FrameStamp {
            connection_epoch: 1,
            layout_revision: 1,
            display_id,
            sequence: 1,
        }
    }

    #[test]
    fn gui_can_swap_buffer_without_changing_snapshot() {
        let mut cache = FrameCache::new(32);
        let mut rgba = image();
        let frame = cache.copy("s1", stamp(0), &rgba, false).unwrap();
        let mut gui = vec![];
        std::mem::swap(&mut rgba.raw, &mut gui);
        gui.fill(0);
        assert_eq!(frame.pixels()[0..4], [1, 2, 3, 255]);
        assert_eq!(frame.stride, 8);
        assert_eq!(frame.format, PixelFormat::Bgra);
        assert_eq!(
            cache
                .read("s1", 0, 1, 1, Some(1))
                .unwrap()
                .newer_than_cursor,
            Some(false)
        );
        assert_eq!(
            cache.read("s1", 0, 2, 1, None).err(),
            Some(FrameError::StaleConnection)
        );
        assert_eq!(
            cache.read("s1", 0, 1, 2, None).err(),
            Some(FrameError::StaleLayout)
        );
    }

    #[test]
    fn readers_cannot_bypass_memory_budget() {
        let mut cache = FrameCache::new(8);
        let held = cache.copy("s1", stamp(0), &image(), false).unwrap();
        assert_eq!(
            cache.copy("s2", stamp(0), &image(), false).err(),
            Some(FrameError::Capacity)
        );
        drop(held);
        assert!(cache.copy("s2", stamp(0), &image(), false).is_ok());
        assert_eq!(
            cache.read("s1", 0, 1, 1, None).err(),
            Some(FrameError::NoFrame)
        );
        assert!(cache.read("s2", 0, 1, 1, None).is_ok());
    }

    #[test]
    fn unread_video_cannot_evict_a_recently_read_display() {
        let mut cache = FrameCache::new(16);
        cache.copy("s1", stamp(0), &image(), false).unwrap();
        cache.copy("s1", stamp(1), &image(), false).unwrap();
        let now = Instant::now();
        cache
            .entries
            .get_mut(&("s1".to_owned(), 0))
            .unwrap()
            .last_read = now - std::time::Duration::from_secs(30);
        cache
            .entries
            .get_mut(&("s1".to_owned(), 1))
            .unwrap()
            .last_read = now;
        cache.copy("s1", stamp(0), &image(), false).unwrap();
        cache.copy("s2", stamp(0), &image(), false).unwrap();
        assert_eq!(
            cache.read("s1", 0, 1, 1, None).err(),
            Some(FrameError::NoFrame)
        );
        assert!(cache.read("s1", 1, 1, 1, None).is_ok());
    }
}
