use super::{
    capture::{self, CaptureError, CaptureOptions, RemoteRect},
    control::Permit,
    error::{BridgeError, Result},
    frames::{FrameError, FrameStamp},
    sessions::{self, SessionHandle},
};
use hbb_common::tokio;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Mapping {
    pub stamp: FrameStamp,
    permit: Permit,
    expires: Instant,
    width: u32,
    height: u32,
    rect: RemoteRect,
}
impl Mapping {
    pub fn check(&self, permit: &Permit) -> Result<()> {
        permit.read_check()?;
        if permit.binding_id() != self.permit.binding_id()
            || permit.authority.session_id != self.permit.authority.session_id
        {
            return Err(BridgeError::new(
                "SNAPSHOT_EXPIRED",
                "Snapshot belongs to a different binding",
            ));
        }
        if Instant::now() >= self.expires {
            return Err(BridgeError::new(
                "SNAPSHOT_EXPIRED",
                "Take another screenshot before using coordinates",
            ));
        }
        let session = sessions::get(&permit.authority.session_id)
            .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
        let state = session.snapshot();
        if state.connection_epoch != self.stamp.connection_epoch
            || state.layout_revision != self.stamp.layout_revision
        {
            return Err(BridgeError::new(
                "DISPLAY_CHANGED",
                "Connection or display layout changed; capture again",
            ));
        }
        Ok(())
    }
    pub fn point(&self, x: i32, y: i32) -> Result<(i32, i32)> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return Err(BridgeError::invalid("Position is outside the screenshot"));
        }
        let convert = |p: i32, image: u32, origin: i32, size: u32| -> Result<i32> {
            let offset = (((p as u64 * 2 + 1) * size as u64) / (image as u64 * 2))
                .min(size.saturating_sub(1) as u64);
            i32::try_from(origin as i64 + offset as i64)
                .map_err(|_| BridgeError::invalid("Remote coordinate overflow"))
        };
        Ok((
            convert(x, self.width, self.rect.x, self.rect.width)?,
            convert(y, self.height, self.rect.y, self.rect.height)?,
        ))
    }
}
fn mappings() -> &'static Mutex<HashMap<String, VecDeque<(String, Mapping)>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, VecDeque<(String, Mapping)>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
pub fn mapping(permit: &Permit, id: &str) -> Result<Mapping> {
    let value = mappings()
        .lock()
        .unwrap()
        .get(permit.binding_id())
        .and_then(|entries| {
            entries
                .iter()
                .find(|(key, _)| key == id)
                .map(|(_, v)| v.clone())
        })
        .ok_or_else(|| {
            BridgeError::new("SNAPSHOT_EXPIRED", "Snapshot mapping is absent or expired")
        })?;
    value.check(permit)?;
    Ok(value)
}
pub fn tick() {
    mappings().lock().unwrap().retain(|_, values| {
        values.retain(|(_, v)| v.expires > Instant::now() && v.permit.read_check().is_ok());
        !values.is_empty()
    });
}
pub fn primary(session: &SessionHandle) -> Option<usize> {
    let snapshot = session.snapshot();
    let online = snapshot
        .displays
        .iter()
        .filter(|d| d.online)
        .collect::<Vec<_>>();
    if online.len() == 1 {
        return Some(online[0].id);
    }
    // Windows and macOS define the main monitor's upper-left as desktop (0,0).
    // The 1.4.9 wire protocol has no primary flag; other platforms need an explicit ID.
    if matches!(snapshot.platform.as_deref(), Some("Windows" | "Mac OS")) {
        let mut origin = online.into_iter().filter(|d| d.x == 0 && d.y == 0);
        let id = origin.next()?.id;
        if origin.next().is_none() {
            return Some(id);
        }
    }
    None
}
pub fn display(session: &SessionHandle, name: &str) -> Result<usize> {
    let id = if name == "primary" {
        primary(session)
    } else {
        name.parse().ok()
    };
    id.filter(|id| session.snapshot().displays.iter().any(|d| d.id == *id && d.online)).ok_or_else(|| BridgeError::new("DISPLAY_CHANGED", "Display is unavailable or primary cannot be identified; select an actual ID from session_get"))
}
pub struct Observation {
    pub data: Value,
    pub png: Option<Vec<u8>>,
    pub unchanged: bool,
}
pub async fn capture(
    permit: &Permit,
    display_name: &str,
    after: Option<u64>,
    wait_ms: u64,
    width: u32,
    height: u32,
) -> Result<Observation> {
    permit.read_check()?;
    if wait_ms > 30_000
        || !(1..=3840).contains(&width)
        || !(1..=3840).contains(&height)
        || (after.is_some() && display_name == "primary")
    {
        return Err(BridgeError::invalid(
            "Invalid capture limits or primary alias with a frame cursor",
        ));
    }
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
    let display = display(&session, display_name)?;
    session.set_capture_enabled(true);
    if let Some(core) = sessions::core(&permit.authority.session_id) {
        if session.snapshot().state != super::sessions::ConnectionState::Disconnected {
            super::subscriptions::need(permit, display as i32)?;
            core.refresh_video(display as i32);
        }
    }
    let mut changed = session.subscribe_frames();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        permit.read_check()?;
        let result = capture::capture(
            &session,
            display,
            CaptureOptions {
                max_width: width,
                max_height: height,
                after_frame_seq: after,
            },
        )
        .await;
        match result {
            Ok(Some(image)) => {
                permit.read_check()?;
                let id = format!("snap_{}", Uuid::new_v4());
                let mapping = Mapping {
                    stamp: image.stamp,
                    permit: permit.clone(),
                    expires: Instant::now() + Duration::from_secs(30),
                    width: image.image_width,
                    height: image.image_height,
                    rect: image.remote_rect,
                };
                let data = json!({"session_ref":session.control().view().session_ref,"frame":{"snapshot_id":id,"display_id":display.to_string(),"connection_epoch":image.stamp.connection_epoch.to_string(),"layout_revision":image.stamp.layout_revision.to_string(),"frame_seq":image.stamp.sequence.to_string(),"received_at":image.received_at,"age_ms":image.age_ms,"is_stale":image.is_stale,"is_new":image.is_new,"image_width":image.image_width,"image_height":image.image_height,"remote_rect":{"x":image.remote_rect.x,"y":image.remote_rect.y,"width":image.remote_rect.width,"height":image.remote_rect.height},"cursor_composited":false,"mapping_expires_at":chrono::Utc::now()+chrono::Duration::seconds(30)},"image_content_index":1});
                let mut cache = mappings().lock().unwrap();
                let entries = cache.entry(permit.binding_id().to_owned()).or_default();
                entries.retain(|(_, v)| v.expires > Instant::now());
                while entries.len() >= 64 {
                    entries.pop_front();
                }
                entries.push_back((id, mapping));
                return Ok(Observation {
                    data,
                    png: Some(image.png),
                    unchanged: false,
                });
            }
            Ok(None) | Err(CaptureError::Frame(FrameError::NoFrame)) => {}
            Err(CaptureError::ImageTooLarge) => {
                return Err(BridgeError::new(
                    "IMAGE_TOO_LARGE",
                    "PNG exceeds 8 MiB; reduce max_width or max_height",
                ))
            }
            Err(CaptureError::WrongSessionKind) => {
                return Err(BridgeError::new(
                    "WRONG_SESSION_KIND",
                    "Capture requires a desktop session",
                ))
            }
            Err(_) => {
                return Err(BridgeError::new(
                    "NO_FRAME",
                    "A CPU-readable frame matching the current display layout is not available",
                ))
            }
        }
        if tokio::time::timeout_at(deadline, changed.changed())
            .await
            .is_err()
        {
            if after.is_some() && session.read_frame(display, None).is_ok() {
                return Ok(Observation {
                    data: json!({"session_ref":session.control().view().session_ref}),
                    png: None,
                    unchanged: true,
                });
            }
            return Err(BridgeError::new(
                "NO_FRAME",
                "No decoded frame arrived within the wait budget",
            ));
        }
    }
}
