//! Bounded, authority-checked requests executed by a specific local desktop view.
use super::{
    control::Permit,
    displays,
    error::{BridgeError, Result},
    sessions,
};
use hbb_common::tokio;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "setting", rename_all = "snake_case", deny_unknown_fields)]
pub enum Setting {
    Scale {
        mode: ScaleMode,
        percent: Option<u16>,
    },
    IndividualWindows {
        enabled: bool,
    },
    UseAllLocalDisplays {
        enabled: bool,
    },
    ShowRemoteCursor {
        enabled: bool,
    },
    FollowRemoteCursor {
        enabled: bool,
    },
    FollowRemoteFocus {
        enabled: bool,
    },
    ScaleCursor {
        enabled: bool,
    },
    FollowAiDisplay {
        enabled: bool,
    },
    ToolbarPinned {
        enabled: bool,
    },
    Fullscreen {
        enabled: bool,
    },
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    Original,
    Adaptive,
    Custom,
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowAction {
    Show,
    Close,
    OpenDisplay { display_id: String },
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "setting", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectionSetting {
    Quality {
        preset: QualityPreset,
        quality: Option<u16>,
        fps: Option<u16>,
    },
    Codec {
        preference: CodecPreference,
    },
    TrueColor {
        enabled: bool,
    },
    AudioMuted {
        enabled: bool,
    },
    QualityOverlay {
        enabled: bool,
    },
    LockAfterEnd {
        enabled: bool,
    },
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    Best,
    Balanced,
    Low,
    Custom,
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CodecPreference {
    Auto,
    Vp8,
    Vp9,
    Av1,
    H264,
    H265,
}
impl ConnectionSetting {
    fn validate(&self) -> Result<()> {
        if let Self::Quality {
            preset,
            quality,
            fps,
        } = self
        {
            match preset {
                QualityPreset::Custom if quality.is_some_and(|q| (10..=2000).contains(&q)) && fps.is_none_or(|f| (5..=120).contains(&f)) => {},
                QualityPreset::Best | QualityPreset::Balanced | QualityPreset::Low if quality.is_none() && fps.is_none() => {},
                _ => return Err(BridgeError::invalid("Custom quality requires quality 10..2000 and optional fps 5..120; presets omit quality/fps")),
            }
        }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Get,
    ConnectionGet,
    ConnectionSet { change: ConnectionSetting },
    Set { change: Setting },
    Window { action: WindowAction },
}
impl Command {
    fn write(&self) -> bool {
        !matches!(self, Self::Get | Self::ConnectionGet)
    }
    fn validate(&self) -> Result<()> {
        if let Self::ConnectionSet { change } = self {
            return change.validate();
        }
        if let Self::Set {
            change: Setting::Scale { mode, percent },
        } = self
        {
            match (mode, percent) {
                (ScaleMode::Custom, Some(5..=1000))
                | (ScaleMode::Original | ScaleMode::Adaptive, None) => {}
                _ => {
                    return Err(BridgeError::invalid(
                        "Custom scale requires percent 5..1000; other modes omit percent",
                    ))
                }
            }
        }
        Ok(())
    }
}
struct Request {
    permit: Permit,
    view: Uuid,
    command: Value,
    write: bool,
    claimed: bool,
    result: Option<Result<Value>>,
    deadline: Instant,
}
fn requests() -> &'static Mutex<HashMap<String, Request>> {
    static REQUESTS: OnceLock<Mutex<HashMap<String, Request>>> = OnceLock::new();
    REQUESTS.get_or_init(Default::default)
}
fn check(r: &Request, view: Uuid) -> Result<()> {
    if r.view != view || Instant::now() >= r.deadline {
        return Err(BridgeError::new(
            "GUI_REQUEST_EXPIRED",
            "View request expired or belongs to another window",
        ));
    }
    displays::check(&r.permit, r.write)?;
    if !sessions::core(&r.permit.authority.session_id).is_some_and(|c| {
        c.ui_handler
            .automation_display_views()
            .iter()
            .any(|v| v["ui_session_id"] == view.to_string())
    }) {
        return Err(BridgeError::new(
            "GUI_UNAVAILABLE",
            "Target desktop view closed",
        ));
    }
    Ok(())
}
/// Claim once in the destination engine. Subsequent guards recheck authority after awaits.
pub fn claim(id: &str, view: Uuid) -> Result<Value> {
    let mut all = requests().lock().unwrap();
    let r = all
        .get_mut(id)
        .ok_or_else(|| BridgeError::new("GUI_REQUEST_EXPIRED", "View request absent"))?;
    check(r, view)?;
    if r.claimed {
        return Err(BridgeError::new(
            "GUI_REQUEST_EXPIRED",
            "View request already claimed",
        ));
    }
    r.claimed = true;
    Ok(r.command.clone())
}
pub fn guard(id: &str, view: Uuid) -> Result<()> {
    let all = requests().lock().unwrap();
    let r = all
        .get(id)
        .ok_or_else(|| BridgeError::new("GUI_REQUEST_EXPIRED", "View request absent"))?;
    if !r.claimed || r.result.is_some() {
        return Err(BridgeError::new(
            "GUI_REQUEST_EXPIRED",
            "View request is not executing",
        ));
    }
    check(r, view)
}
pub fn complete(id: &str, view: Uuid, value: &str) -> bool {
    let mut all = requests().lock().unwrap();
    let Some(r) = all.get_mut(id) else {
        return false;
    };
    if r.view != view || !r.claimed || r.result.is_some() {
        return false;
    }
    let active = check(r, view).is_ok();
    r.result = Some(if value.len() > 262144 {
        Err(BridgeError::invalid("GUI response too large"))
    } else {
        serde_json::from_str::<Value>(value)
            .map_err(|_| BridgeError::new("GUI_ERROR", "Invalid GUI response"))
            .and_then(|v| {
                if let Some(code) = v["error"]["code"].as_str() {
                    let code = match code {
                        "CONTROL_EXPIRED" => "CONTROL_EXPIRED",
                        "UNSUPPORTED" => "UNSUPPORTED",
                        "SETTING_CONFLICT" => "SETTING_CONFLICT",
                        "INVALID_ARGUMENT" => "INVALID_ARGUMENT",
                        "DISPLAY_NOT_FOUND" => "DISPLAY_NOT_FOUND",
                        "PERMISSION_DENIED" => "PERMISSION_DENIED",
                        "CODEC_UNKNOWN" => "CODEC_UNKNOWN",
                        _ => "GUI_ERROR",
                    };
                    Err(BridgeError::new(
                        code,
                        v["error"]["message"]
                            .as_str()
                            .unwrap_or("Local view operation failed"),
                    ))
                } else {
                    Ok(v)
                }
            })
    });
    active
}
pub async fn request(
    permit: Permit,
    view: Option<String>,
    command: Command,
    wait_ms: u64,
) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    command.validate()?;
    let s = displays::check(&permit, command.write())?;
    if let Command::Window {
        action: WindowAction::OpenDisplay { display_id },
    } = &command
    {
        let index = display_id
            .parse::<usize>()
            .map_err(|_| BridgeError::invalid("display_id must be numeric"))?;
        if !s.displays.iter().any(|d| d.id == index && d.online) {
            return Err(BridgeError::new(
                "DISPLAY_NOT_FOUND",
                "Display is absent or offline",
            ));
        }
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    let views = core.ui_handler.automation_display_views();
    let view = match view {
        Some(v) => {
            Uuid::parse_str(&v).map_err(|_| BridgeError::invalid("Invalid ui_session_id"))?
        }
        None if views.len() == 1 => views[0]["ui_session_id"]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop view unavailable"))?,
        None if views.is_empty() => {
            return Err(BridgeError::new("GUI_UNAVAILABLE", "No desktop view"))
        }
        None => {
            return Err(BridgeError::new(
                "AMBIGUOUS_VIEW",
                "Choose ui_session_id from displays_get",
            ))
        }
    };
    let id = format!("view_{}", Uuid::new_v4());
    let r = Request {
        permit: permit.clone(),
        view,
        write: command.write(),
        command: serde_json::to_value(command)
            .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Cannot encode view command"))?,
        claimed: false,
        result: None,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    check(&r, view)?;
    {
        let mut all = requests().lock().unwrap();
        all.retain(|_, r| Instant::now() < r.deadline);
        if all.len() >= 128 {
            return Err(BridgeError::new(
                "LIMIT_EXCEEDED",
                "Too many pending view requests",
            ));
        }
        all.insert(id.clone(), r);
    }
    core.ui_handler
        .push_event_to("automation_view", &[("request_id", &id)], &[&view]);
    let until = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let result = requests()
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|r| r.result.take());
        if let Some(result) = result {
            requests().lock().unwrap().remove(&id);
            return result;
        }
        permit.read_check()?;
        if tokio::time::Instant::now() >= until {
            return Ok(
                json!({"confirmed":false,"delivery":"queued","ui_session_id":view.to_string(),"request_id":id,"hint":"Query view settings or displays_get to observe; do not blindly replay a window action."}),
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quality_contract_rejects_ambiguous_presets_and_invalid_ranges() {
        for (preset, quality, fps, valid) in [
            (QualityPreset::Custom, Some(10), Some(5), true),
            (QualityPreset::Custom, Some(2000), Some(120), true),
            (QualityPreset::Custom, None, None, false),
            (QualityPreset::Custom, Some(9), None, false),
            (QualityPreset::Custom, Some(50), Some(121), false),
            (QualityPreset::Balanced, Some(50), None, false),
            (QualityPreset::Best, None, None, true),
        ] {
            assert_eq!(
                ConnectionSetting::Quality {
                    preset,
                    quality,
                    fps
                }
                .validate()
                .is_ok(),
                valid
            );
        }
        assert!(serde_json::from_str::<ConnectionSetting>(
            r#"{"setting":"codec","preference":"h266"}"#
        )
        .is_err());
    }
    #[test]
    fn scale_validation_rejects_ambiguous_and_out_of_range_values() {
        for (mode, percent, ok) in [
            (ScaleMode::Custom, None, false),
            (ScaleMode::Custom, Some(4), false),
            (ScaleMode::Custom, Some(1001), false),
            (ScaleMode::Custom, Some(125), true),
            (ScaleMode::Original, Some(100), false),
            (ScaleMode::Adaptive, None, true),
        ] {
            assert_eq!(
                Command::Set {
                    change: Setting::Scale { mode, percent }
                }
                .validate()
                .is_ok(),
                ok
            );
        }
        assert!(serde_json::from_str::<Setting>(
            r#"{"setting":"fullscreen","enabled":true,"percent":12}"#
        )
        .is_err());
    }
}
