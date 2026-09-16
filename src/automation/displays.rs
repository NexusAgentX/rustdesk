//! Display topology and stock resolution/virtual-monitor requests.
use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions::{self, SessionKind, SessionSnapshot},
    wire,
};
use hbb_common::{
    message_proto::{
        DisplayResolution, Message, Misc, Resolution, SwitchDisplay, ToggleVirtualDisplay,
    },
    tokio,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub fn check(permit: &Permit, write: bool) -> Result<SessionSnapshot> {
    if write {
        permit.check()?;
    } else {
        permit.read_check()?;
    }
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Desktop closed"))?
        .snapshot();
    if s.kind != SessionKind::Desktop {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "Displays require a desktop session",
        ));
    }
    if write && (!s.authenticated || s.state != sessions::ConnectionState::Ready) {
        return Err(BridgeError::new("NOT_READY", "Desktop is not ready"));
    }
    Ok(s)
}
pub fn remote_write_check(permit: &Permit) -> Result<SessionSnapshot> {
    let s = check(permit, true)?;
    if s.permissions.get("keyboard") != Some(&true) {
        return Err(BridgeError::new(
            "PERMISSION_DENIED",
            "Keyboard permission is required",
        ));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    if core.lc.read().unwrap().view_only.v {
        return Err(BridgeError::new("VIEW_ONLY", "Desktop is view-only"));
    }
    Ok(s)
}
fn id(s: &SessionSnapshot, value: &str) -> Result<usize> {
    let id = value
        .parse::<usize>()
        .map_err(|_| BridgeError::invalid("display_id must be a numeric display ID"))?;
    if !s.displays.iter().any(|d| d.id == id && d.online) {
        return Err(BridgeError::new(
            "DISPLAY_NOT_FOUND",
            "Display is absent or offline",
        ));
    }
    Ok(id)
}
fn multi(s: &SessionSnapshot) -> bool {
    s.peer_version
        .as_deref()
        .is_some_and(crate::common::is_support_multi_ui_session)
}
fn packet(misc: Misc) -> Message {
    let mut message = Message::new();
    message.set_misc(misc);
    message
}
pub fn virtual_info(s: &SessionSnapshot) -> Value {
    let p = &s.platform_additions;
    let implementation = p["idd_impl"].as_str();
    let supported = s.platform.as_deref() == Some("Windows")
        && p["is_installed"] == true
        && matches!(implementation, Some("rustdesk_idd" | "amyuni_idd"));
    json!({"supported":supported,"reason":if supported {Value::Null} else {json!("requires_installed_windows_peer_reporting_idd_implementation")},"implementation":implementation,"driver_installed":Value::Null,"driver_installation_may_occur":true,"rustdesk_indices":p["rustdesk_virtual_displays"].as_array().cloned().unwrap_or_default(),"amyuni_count":p["amyuni_virtual_displays"].as_u64().unwrap_or(0),"scope":"remote_machine"})
}
pub fn get(permit: &Permit) -> Result<Value> {
    let s = check(permit, false)?;
    let views = sessions::core(&s.session_id)
        .map(|c| c.ui_handler.automation_display_views())
        .unwrap_or_default();
    Ok(
        json!({"displays":s.displays.iter().map(|d| json!({"id":d.id.to_string(),"name":d.name,"x":d.x,"y":d.y,"width":d.width,"height":d.height,"scale":d.scale,"online":d.online,"original_resolution":d.original_resolution.map(|(w,h)|json!({"width":w,"height":h})),"custom_resolution_supported":d.original_resolution==Some((0,0)),"modes_known":s.resolutions.contains_key(&d.id),"modes":s.resolutions.get(&d.id).map(|m|m.iter().map(|(w,h)|json!({"width":w,"height":h})).collect::<Vec<_>>())})).collect::<Vec<_>>(),"layout_revision":s.layout_revision.to_string(),"remote_current_display":s.current_display.to_string(),"capture_selection":super::subscriptions::selection(permit),"local_views":views,"virtual_displays":virtual_info(&s)}),
    )
}
pub fn modes(permit: &Permit, display: &str) -> Result<Value> {
    let s = check(permit, false)?;
    let id = id(&s, display)?;
    let d = &s.displays[id];
    Ok(
        json!({"display_id":display,"known":s.resolutions.contains_key(&id),"modes":s.resolutions.get(&id).map(|m|m.iter().map(|(w,h)|json!({"width":w,"height":h})).collect::<Vec<_>>()),"current":{"width":d.width,"height":d.height,"scale":d.scale},"original":d.original_resolution.map(|(w,h)|json!({"width":w,"height":h})),"custom_supported":d.original_resolution==Some((0,0)),"unknown_hint":"Switch from another local display to this display to request its stock mode report; reconnect if no other display exists."}),
    )
}
#[derive(Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMode {
    Set,
    RestoreOriginal,
    FitLocal,
}
fn resolution_target(
    s: &SessionSnapshot,
    display: usize,
    mode: ResolutionMode,
    width: Option<i32>,
    height: Option<i32>,
    local: Option<(i32, i32)>,
) -> Result<(i32, i32)> {
    let d = &s.displays[display];
    let target = match mode {
        ResolutionMode::Set => (
            width.ok_or_else(|| BridgeError::invalid("set requires width"))?,
            height.ok_or_else(|| BridgeError::invalid("set requires height"))?,
        ),
        ResolutionMode::RestoreOriginal => d
            .original_resolution
            .filter(|(w, h)| *w > 0 && *h > 0)
            .ok_or_else(|| {
                BridgeError::new(
                    "UNSUPPORTED",
                    "No original physical resolution was reported",
                )
            })?,
        ResolutionMode::FitLocal => local.ok_or_else(|| {
            BridgeError::new("UNSUPPORTED", "Local main-display resolution unavailable")
        })?,
    };
    if !matches!(mode, ResolutionMode::Set) && (width.is_some() || height.is_some()) {
        return Err(BridgeError::invalid("width/height apply only to set"));
    }
    if !(1..=16384).contains(&target.0) || !(1..=16384).contains(&target.1) {
        return Err(BridgeError::invalid(
            "Resolution dimensions must be 1..16384",
        ));
    }
    if !matches!(mode, ResolutionMode::RestoreOriginal) && d.original_resolution != Some((0, 0)) {
        let modes = s.resolutions.get(&display).ok_or_else(|| {
            BridgeError::new(
                "MODES_UNKNOWN",
                "Select this display locally first to obtain supported modes",
            )
        })?;
        if !modes.contains(&target) {
            return Err(BridgeError::new(
                "UNSUPPORTED_RESOLUTION",
                "Requested size is not a reported mode; fit_local requires an exact match",
            ));
        }
    }
    Ok(target)
}
pub async fn resolution(
    permit: Permit,
    display: &str,
    mode: ResolutionMode,
    width: Option<i32>,
    height: Option<i32>,
    wait_ms: u64,
) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s = remote_write_check(&permit)?;
    let display = id(&s, display)?;
    if !multi(&s) && display != s.current_display {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "This peer can change only its current display",
        ));
    }
    let local = if matches!(mode, ResolutionMode::FitLocal) {
        tokio::task::spawn_blocking(|| {
            crate::display_service::try_get_displays()
                .ok()
                .and_then(|d| d.first().map(|d| (d.width() as i32, d.height() as i32)))
        })
        .await
        .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Local display query failed"))?
    } else {
        None
    };
    let (width, height) = resolution_target(&s, display, mode, width, height, local)?;
    let mut misc = Misc::new();
    let resolution = Resolution {
        width,
        height,
        ..Default::default()
    };
    if multi(&s) {
        misc.set_change_display_resolution(DisplayResolution {
            display: display as i32,
            resolution: Some(resolution).into(),
            ..Default::default()
        });
    } else {
        misc.set_change_resolution(resolution);
    }
    wire::send(permit.clone(), packet(misc)).await?;
    let observed = wait_for(&permit, wait_ms, |s| {
        s.displays
            .iter()
            .find(|d| d.id == display)
            .is_some_and(|d| {
                (
                    (d.width as f64 / d.scale.max(1.0)).round() as i32,
                    (d.height as f64 / d.scale.max(1.0)).round() as i32,
                ) == (width, height)
            })
    })
    .await?;
    Ok(
        json!({"delivery":"sent","confirmed":observed,"requested":{"display_id":display.to_string(),"width":width,"height":height},"scope":"remote_machine","state":get(&permit)?}),
    )
}
async fn wait_for(
    permit: &Permit,
    wait_ms: u64,
    predicate: impl Fn(&SessionSnapshot) -> bool,
) -> Result<bool> {
    let until = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        permit.read_check()?;
        let s = check(permit, false)?;
        if s.connection_epoch != permit.epoch {
            return Err(BridgeError::new(
                "CONTROL_EXPIRED",
                "Connection changed while observing the display request",
            ));
        }
        if predicate(&s) {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= until {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
#[derive(Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VirtualAction {
    Add,
    Remove,
    RemoveAll,
}
pub async fn virtual_change(
    permit: Permit,
    action: VirtualAction,
    index: Option<i32>,
    wait_ms: u64,
) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s = remote_write_check(&permit)?;
    let info = virtual_info(&s);
    if info["supported"] != true {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "Peer does not report installed Windows virtual-display support",
        ));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    if core.lc.read().unwrap().privacy_mode.v {
        return Err(BridgeError::new(
            "PRIVACY_MODE_ACTIVE",
            "Disable privacy mode before managing virtual displays",
        ));
    }
    let amyuni = info["implementation"] == "amyuni_idd";
    let count = info["amyuni_count"].as_u64().unwrap_or(0);
    let display = if matches!(action, VirtualAction::RemoveAll) {
        if index.is_some() {
            return Err(BridgeError::invalid("remove_all does not accept index"));
        }
        -1
    } else if amyuni {
        if index.is_some() {
            return Err(BridgeError::invalid(
                "Amyuni adds/removes one monitor; omit index",
            ));
        }
        if matches!(action, VirtualAction::Add) && count >= 4 {
            return Err(BridgeError::new(
                "LIMIT_EXCEEDED",
                "Amyuni supports at most four virtual displays",
            ));
        }
        0
    } else {
        let i = index.ok_or_else(|| BridgeError::invalid("RustDesk IDD requires index 1..4"))?;
        if !(1..=4).contains(&i) {
            return Err(BridgeError::invalid("index must be 1..4"));
        }
        i
    };
    let on = matches!(action, VirtualAction::Add);
    let before = info["rustdesk_indices"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let already = if amyuni {
        !on && count == 0
    } else if display < 0 {
        before.is_empty()
    } else {
        before.contains(&json!(display)) == on
    };
    if already {
        return Ok(json!({"delivery":"not_needed","confirmed":true,"state":get(&permit)?}));
    }
    let mut misc = Misc::new();
    misc.set_toggle_virtual_display(ToggleVirtualDisplay {
        display,
        on,
        ..Default::default()
    });
    wire::send(permit.clone(), packet(misc)).await?;
    let confirmed = wait_for(&permit, wait_ms, |s| {
        let after = virtual_info(s);
        if amyuni {
            let n = after["amyuni_count"].as_u64().unwrap_or(0);
            if display < 0 {
                n == 0
            } else if on {
                n > count
            } else {
                n < count
            }
        } else {
            let indices = after["rustdesk_indices"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if display < 0 {
                indices.is_empty()
            } else {
                indices.contains(&json!(display)) == on
            }
        }
    })
    .await?;
    Ok(
        json!({"delivery":"sent","confirmed":confirmed,"driver_installation_may_occur":on,"scope":"remote_machine","state":get(&permit)?}),
    )
}

#[derive(Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectionTarget {
    Capture,
    LocalView,
}
struct ViewRequest {
    permit: Permit,
    view: uuid::Uuid,
    displays: Vec<i32>,
    all: bool,
    applied: bool,
    deadline: Instant,
}
fn requests() -> &'static Mutex<HashMap<String, ViewRequest>> {
    static REQUESTS: OnceLock<Mutex<HashMap<String, ViewRequest>>> = OnceLock::new();
    REQUESTS.get_or_init(Default::default)
}
// Invoked by the destination Flutter view, so a late event cannot apply after revocation.
pub fn apply_view(request_id: &str, view: uuid::Uuid) -> Option<i32> {
    let mut requests = requests().lock().unwrap();
    let r = requests.get_mut(request_id)?;
    if r.view != view || r.applied || Instant::now() > r.deadline || check(&r.permit, true).is_err()
    {
        return None;
    }
    let core = sessions::core(&r.permit.authority.session_id)?;
    if !core
        .ui_handler
        .automation_set_display_view(&view, &r.displays)
    {
        return None;
    }
    r.applied = true;
    sessions::get(&r.permit.authority.session_id)?.selection_changed();
    Some(if r.all { -1 } else { r.displays[0] })
}
pub async fn select(
    permit: Permit,
    display: &str,
    target: SelectionTarget,
    view: Option<String>,
    wait_ms: u64,
) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s = check(&permit, true)?;
    let all = display == "all";
    let selected = if all {
        s.displays
            .iter()
            .filter(|d| d.online)
            .map(|d| d.id as i32)
            .collect::<Vec<_>>()
    } else {
        vec![id(&s, display)? as i32]
    };
    if selected.is_empty() {
        return Err(BridgeError::new("DISPLAY_NOT_FOUND", "No online displays"));
    }
    if !multi(&s) {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "Display selection requires multi-display protocol support",
        ));
    }
    let applied = match target {
        SelectionTarget::Capture => {
            if view.is_some() {
                return Err(BridgeError::invalid(
                    "ui_session_id applies only to local_view",
                ));
            }
            let msg = super::subscriptions::set_ai(&permit, &selected)?;
            wire::send(permit.clone(), msg).await?;
            sessions::get(&s.session_id)
                .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Desktop closed"))?
                .selection_changed();
            true
        }
        SelectionTarget::LocalView => {
            let view = match view {
                Some(v) => v,
                None if s.ui_session_ids.len() == 1 => s
                    .ui_session_ids
                    .iter()
                    .next()
                    .cloned()
                    .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "No view"))?,
                None => {
                    return Err(BridgeError::new(
                        "AMBIGUOUS_VIEW",
                        "Specify ui_session_id from local_views",
                    ))
                }
            };
            if !s.ui_session_ids.contains(&view) {
                return Err(BridgeError::invalid(
                    "ui_session_id does not belong to this session",
                ));
            }
            let view = uuid::Uuid::parse_str(&view)
                .map_err(|_| BridgeError::invalid("Invalid ui_session_id"))?;
            let request = format!("display_{}", uuid::Uuid::new_v4());
            {
                let mut requests = requests().lock().unwrap();
                requests.retain(|_, r| Instant::now() < r.deadline);
                if requests
                    .values()
                    .any(|r| r.permit.authority.session_id == s.session_id)
                {
                    return Err(BridgeError::new(
                        "BUSY",
                        "Display selection is already in progress",
                    ));
                }
                requests.insert(
                    request.clone(),
                    ViewRequest {
                        permit: permit.clone(),
                        view,
                        displays: selected.clone(),
                        all,
                        applied: false,
                        deadline: Instant::now() + Duration::from_secs(30),
                    },
                );
            }
            struct Cleanup(String);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    requests().lock().unwrap().remove(&self.0);
                }
            }
            let _cleanup = Cleanup(request.clone());
            let core = sessions::core(&s.session_id)
                .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
            core.ui_handler.push_event_to(
                "automation_display_select",
                &[("request_id", request.clone())],
                &[&view],
            );
            let until = tokio::time::Instant::now() + Duration::from_millis(wait_ms.max(1000));
            let applied = loop {
                permit.check()?;
                if requests()
                    .lock()
                    .unwrap()
                    .get(&request)
                    .is_some_and(|r| r.applied)
                {
                    break true;
                }
                if tokio::time::Instant::now() >= until {
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            if applied {
                let gui = core.ui_handler.automation_display_ids();
                let msg = super::subscriptions::set_gui(&permit, &gui)?;
                wire::send(permit.clone(), msg).await?;
                if selected.len() == 1 {
                    // Do not use Session::switch_display: it also applies saved custom resolution.
                    let mut misc = Misc::new();
                    misc.set_switch_display(SwitchDisplay {
                        display: selected[0],
                        ..Default::default()
                    });
                    wire::send(permit.clone(), packet(misc)).await?;
                }
            }
            applied
        }
    };
    Ok(
        json!({"delivery":if applied{"local_selection_applied"}else{"gui_timeout"},"confirmed":applied,"state":get(&permit)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> SessionSnapshot {
        let mut s =
            sessions::SessionHandle::new("peer".into(), SessionKind::Desktop, Default::default())
                .snapshot();
        s.displays.push(sessions::Display {
            id: 0,
            name: "display".into(),
            x: 0,
            y: -563,
            width: 1920,
            height: 1080,
            scale: 1.0,
            cursor_embedded: false,
            online: true,
            original_resolution: Some((1920, 1080)),
        });
        s
    }
    #[test]
    fn physical_mode_validation_never_guesses_support_or_fits_to_nearest() {
        let mut s = snapshot();
        assert!(
            resolution_target(&s, 0, ResolutionMode::Set, Some(1280), Some(720), None).is_err()
        );
        assert_eq!(
            resolution_target(&s, 0, ResolutionMode::RestoreOriginal, None, None, None).unwrap(),
            (1920, 1080)
        );
        s.resolutions.insert(0, vec![(1920, 1080), (1280, 720)]);
        assert_eq!(
            resolution_target(&s, 0, ResolutionMode::RestoreOriginal, None, None, None).unwrap(),
            (1920, 1080)
        );
        assert!(resolution_target(
            &s,
            0,
            ResolutionMode::FitLocal,
            None,
            None,
            Some((1800, 1000))
        )
        .is_err());
        assert_eq!(
            resolution_target(
                &s,
                0,
                ResolutionMode::FitLocal,
                None,
                None,
                Some((1280, 720))
            )
            .unwrap(),
            (1280, 720)
        );
        s.displays[0].original_resolution = Some((0, 0));
        assert_eq!(
            resolution_target(&s, 0, ResolutionMode::Set, Some(1800), Some(1000), None).unwrap(),
            (1800, 1000)
        );
        assert!(
            resolution_target(&s, 0, ResolutionMode::RestoreOriginal, None, None, None).is_err()
        );
        assert!(resolution_target(&s, 0, ResolutionMode::Set, Some(-1), Some(1000), None).is_err());
    }
    #[test]
    fn idd_implementation_does_not_claim_a_driver_is_installed() {
        let mut s = snapshot();
        assert_eq!(virtual_info(&s)["supported"], false);
        s.platform = Some("Windows".into());
        s.platform_additions = json!({"is_installed":true,"idd_impl":"amyuni_idd"});
        assert_eq!(virtual_info(&s)["supported"], true);
        assert!(virtual_info(&s)["driver_installed"].is_null());
        assert_eq!(virtual_info(&s)["driver_installation_may_occur"], true);
        s.platform_additions["is_installed"] = json!(false);
        assert_eq!(virtual_info(&s)["supported"], false);
    }
}
