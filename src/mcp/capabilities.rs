//! Facts about implemented operations. Never infer remote grants from an unknown value.
use crate::automation::{
    control::{ControlView, Mode},
    sessions::{ConnectionState, SessionHandle, SessionKind, SessionSnapshot},
};
use serde_json::{json, Value};

fn capability(
    session: &SessionSnapshot,
    control: &ControlView,
    supported: Option<bool>,
    permission: Option<bool>,
    write: bool,
    ready: bool,
    tools: &[&str],
) -> Value {
    let mut blockers = Vec::new();
    match supported {
        Some(false) => blockers.push("unsupported_for_session"),
        None => blockers.push("support_unknown"),
        Some(true) => {}
    }
    match permission {
        Some(false) => blockers.push("permission_denied"),
        None => blockers.push("permission_unknown"),
        Some(true) => {}
    }
    if write && control.mode != Mode::Ai {
        blockers.push("human_control");
    }
    if write && control.transitioning {
        blockers.push("control_transition");
    }
    if ready && (!session.authenticated || session.state != ConnectionState::Ready) {
        blockers.push("not_ready");
    }
    if write && session.state == ConnectionState::Closed {
        blockers.push("session_closed");
    }
    json!({"implemented":true,"supported":supported,"allowed":permission,
        "available":blockers.is_empty(),"blockers":blockers,"tools":tools,
        "scope":"session","requires_ai_control":write})
}

pub(super) fn view(session: &SessionHandle) -> Value {
    let s = session.snapshot();
    let c = session.control().view();
    let desktop = s.kind == SessionKind::Desktop;
    let terminal = if s.kind == SessionKind::Terminal {
        s.terminal_supported
    } else {
        Some(false)
    };
    let cap = |supported, permission, write, ready, tools: &[&str]| {
        capability(&s, &c, supported, permission, write, ready, tools)
    };
    let file_clipboard_supported = desktop && cfg!(any(windows, feature="unix-file-copy-paste")) && s.peer_version.as_deref().is_some_and(|v| hbb_common::get_version_number(v) >= hbb_common::get_version_number("1.3.8"));
    let file_clipboard_permission = match (s.permissions.get("file"), s.permissions.get("keyboard")) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    };
    let mut result = json!({
        "session_ref":c.session_ref,
        "peer":{"platform":s.platform,"version":s.peer_version,"authenticated":s.authenticated,
            "connection_state":crate::automation::api::state_name(s.state),"permissions":s.permissions},
        "capabilities":{
            "session_read":cap(Some(true),Some(true),false,false,&["rd_session_get","rd_capabilities_get"]),
            "session_lifecycle":cap(Some(true),Some(true),true,false,&["rd_session_disconnect","rd_session_reconnect","rd_session_close"]),
            "connection_settings_read":cap(Some(desktop),Some(true),false,false,&["rd_connection_settings_get"]),
            "connection_settings_write":cap(Some(desktop),Some(true),true,true,&["rd_connection_settings_set"]),
            "local_view_read":cap(Some(desktop),Some(true),false,false,&["rd_view_settings_get"]),
            "local_view_write":cap(Some(desktop),Some(true),true,true,&["rd_view_settings_set","rd_view_window"]),
            "display_read":cap(Some(desktop),Some(true),false,false,&["rd_displays_get","rd_display_modes_get"]),
            "display_select":cap(Some(desktop && s.peer_version.as_deref().is_some_and(crate::common::is_support_multi_ui_session)),Some(true),true,true,&["rd_display_select"]),
            "display_resolution":cap(Some(desktop),s.permissions.get("keyboard").copied(),true,true,&["rd_display_resolution_set"]),
            "virtual_display":cap(Some(desktop && crate::automation::displays::virtual_info(&s)["supported"] == true),s.permissions.get("keyboard").copied(),true,true,&["rd_virtual_display_set"]),
            "recording_read":cap(Some(desktop),Some(true),false,false,&["rd_recording_get"]),
            "recording_write":cap(Some(desktop),s.permissions.get("recording").copied(),true,true,&["rd_recording_set"]),
            "chat_read":cap(Some(desktop),Some(true),false,false,&["rd_chat_read"]),
            "chat_send":cap(Some(desktop),Some(true),true,true,&["rd_chat_send"]),
            "security_read":cap(Some(desktop),Some(true),false,false,&["rd_security_get"]),
            "input_block":cap(Some(desktop && s.platform.as_deref()==Some("Windows")),s.permissions.get("keyboard").copied().zip(s.permissions.get("block_input").copied()).map(|(a,b)|a&&b),true,true,&["rd_input_block_set"]),
            "privacy_mode":cap(Some(desktop && s.security.privacy_supported==Some(true) && !crate::automation::security::implementations(&s).is_empty()),s.permissions.get("keyboard").copied().zip(s.permissions.get("privacy_mode").copied()).map(|(a,b)|a&&b),true,true,&["rd_privacy_set"]),
            "elevation":cap(Some(desktop && s.platform.as_deref()==Some("Windows") && s.security.sas_enabled==Some(false) && s.security.portable_service_running!=Some(true) && s.platform_additions["is_installed"]!=true),s.permissions.get("keyboard").copied(),true,true,&["rd_session_elevate"]),
            "os_password":cap(Some(desktop),s.permissions.get("keyboard").copied(),true,true,&["rd_os_password_input"]),
            "ctrl_alt_del":cap(Some(desktop && (s.platform.as_deref()==Some("Linux") || (s.platform.as_deref()==Some("Windows") && s.security.sas_enabled==Some(true)))),s.permissions.get("keyboard").copied(),true,true,&["rd_ctrl_alt_del"]),
            "screen_refresh":cap(Some(desktop),Some(true),true,true,&["rd_screen_refresh"]),
            "original_screenshot":cap(Some(desktop && s.peer_version.as_deref().is_some_and(|v| crate::common::is_support_screenshot_num(hbb_common::get_version_number(v)))),Some(true),true,true,&["rd_screen_capture"]),
            "session_lock":cap(Some(desktop),s.permissions.get("keyboard").copied(),true,true,&["rd_session_lock"]),
            "session_restart":cap(Some(desktop && matches!(s.platform.as_deref(),Some("Windows" | "Linux" | "Mac OS"))),s.permissions.get("restart").copied(),true,true,&["rd_session_restart"]),
            "screen_capture":cap(Some(desktop),Some(true),false,true,&["rd_screen_capture"]),
            "file_read":cap(Some(s.kind == SessionKind::FileTransfer),s.permissions.get("file").copied(),false,true,&["rd_file_list","rd_file_jobs","rd_file_job_get"]),
            "file_write":cap(Some(s.kind == SessionKind::FileTransfer),s.permissions.get("file").copied(),true,true,&["rd_file_manage","rd_file_transfer","rd_file_job_cancel","rd_file_conflict_resolve"]),
            "file_resume":cap(Some(s.kind == SessionKind::FileTransfer && s.peer_version.as_deref().is_some_and(crate::is_support_file_transfer_resume)),s.permissions.get("file").copied(),true,true,&["rd_file_job_resume"]),
            "file_clipboard_settings_read":cap(Some(desktop),Some(true),false,false,&["rd_file_clipboard_get"]),
            "file_clipboard_settings_write":cap(Some(file_clipboard_supported),file_clipboard_permission,true,true,&["rd_file_clipboard_set"]),
            "file_clipboard":cap(Some(file_clipboard_supported),file_clipboard_permission,true,true,&["rd_file_clipboard_copy","rd_file_clipboard_paste","rd_file_clipboard_cancel"]),
            "text_clipboard":cap(Some(desktop),s.permissions.get("clipboard").copied(),false,true,&["rd_clipboard_read"]),
            "clipboard_settings_read":cap(Some(desktop),Some(true),false,false,&["rd_clipboard_settings_get"]),
            "clipboard_settings_write":cap(Some(desktop),s.permissions.get("clipboard").copied(),true,true,&["rd_clipboard_settings_set"]),
            "text_clipboard_write":cap(Some(desktop),s.permissions.get("clipboard").copied(),true,true,&["rd_clipboard_write"]),
            "keyboard_mouse":cap(Some(desktop),s.permissions.get("keyboard").copied(),true,true,&["rd_input_send","rd_clipboard_type"]),
            "terminal_read":cap(terminal,Some(true),false,true,&["rd_terminal_list","rd_terminal_read"]),
            "terminal_write":cap(terminal,Some(true),true,true,&["rd_terminal_create","rd_terminal_write","rd_terminal_resize","rd_terminal_close"])
        },
        "contract":{
            "version":1,
            "availability":"Eligibility to attempt a call, not a guarantee that a frame, terminal or application result exists. Tools revalidate at execution time.",
            "operation_query":"rd_operation_get",
            "operation_scope":"mcp_client",
            "operation_retention_seconds":300,
            "operation_limit":256,
            "wait_timeout":"Observation timeout does not cancel or replay a previously sent request. A stored pending result has unknown final outcome; query current session or terminal state.",
            "completion":"completed describes the individual tool contract; input delivery is not remote application completion.",
            "settings":{"write_mode":"explicit_value","scope_required":true,"scopes":["session","binding","peer_preference","global","local_window","remote_machine"]},
            "credentials":"Never included in operation arguments or capability results; authentication submissions are single-use; TCP listeners retain isolated login state in memory until closed.",
            "future_features":"Capabilities list only implemented MCP operations. GUI-only features are not promises of MCP support."
        }
    });
    result["capabilities"]["tcp_tunnels_read"]=cap(Some(s.kind==SessionKind::TcpTunnel),Some(true),false,false,&["rd_tunnel_list"]);
    result["capabilities"]["tcp_tunnels_write"]=cap(Some(s.kind==SessionKind::TcpTunnel),Some(s.state==ConnectionState::Ready),true,false,&["rd_tunnel_add","rd_tunnel_remove","rd_tunnel_authenticate"]);
    if desktop {
        if let Some(core) = crate::automation::sessions::core(&s.session_id) {
            let lc = core.lc.read().unwrap();
            for key in ["text_clipboard", "text_clipboard_write"] {
                if lc.disable_clipboard.v || lc.view_only.v {
                    result["capabilities"][key]["available"] = json!(false);
                    if let Some(blockers) = result["capabilities"][key]["blockers"].as_array_mut() {
                        blockers.push(json!("clipboard_disabled"));
                    }
                }
            }
            if lc.privacy_mode.v {
                result["capabilities"]["virtual_display"]["available"] = json!(false);
                if let Some(blockers) = result["capabilities"]["virtual_display"]["blockers"].as_array_mut() { blockers.push(json!("privacy_mode_active")); }
            }
            if lc.view_only.v || !lc.enable_file_copy_paste.v {
                result["capabilities"]["file_clipboard"]["available"] = json!(false);
                if let Some(blockers) = result["capabilities"]["file_clipboard"]["blockers"].as_array_mut() { blockers.push(json!("file_clipboard_disabled")); }
            }
            if lc.view_only.v {
                for key in ["keyboard_mouse", "clipboard_settings_write", "file_clipboard_settings_write", "display_resolution", "virtual_display", "session_lock", "input_block", "privacy_mode", "elevation", "os_password", "ctrl_alt_del"] {
                    result["capabilities"][key]["available"] = json!(false);
                    if let Some(blockers) = result["capabilities"][key]["blockers"].as_array_mut() {
                        blockers.push(json!("view_only"));
                    }
                }
            }
        }
    }
    for key in ["display_resolution", "virtual_display", "session_lock", "session_restart", "input_block", "privacy_mode", "elevation", "os_password", "ctrl_alt_del"] { result["capabilities"][key]["scope"] = json!("remote_machine"); }
    for key in ["clipboard_settings_read", "clipboard_settings_write", "file_clipboard_settings_read", "file_clipboard_settings_write"] {
        result["capabilities"][key]["scope"] = json!("peer_preference");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::control::Agent;

    #[test]
    fn unknown_denied_and_human_control_are_distinct() {
        let session = SessionHandle::new("peer".into(), SessionKind::Desktop, Default::default());
        let mut s = session.snapshot();
        let mut c = session.control().view();
        s.authenticated = true;
        s.state = ConnectionState::Ready;
        c.mode = Mode::Ai;
        let get = |s: &SessionSnapshot, c: &ControlView, p| {
            capability(s, c, Some(true), p, true, true, &["rd_input_send"])
        };
        assert_eq!(get(&s, &c, None)["blockers"], json!(["permission_unknown"]));
        assert_eq!(
            get(&s, &c, Some(false))["blockers"],
            json!(["permission_denied"])
        );
        assert_eq!(get(&s, &c, Some(true))["available"], true);
        c.mode = Mode::Human;
        assert_eq!(
            get(&s, &c, Some(true))["blockers"],
            json!(["human_control"])
        );
        s.state = ConnectionState::Disconnected;
        assert_eq!(
            get(&s, &c, Some(true))["blockers"],
            json!(["human_control", "not_ready"])
        );
    }

    #[test]
    fn wrong_kind_and_unnegotiated_terminal_do_not_claim_support() {
        let desktop = SessionHandle::new("peer".into(), SessionKind::Desktop, Default::default());
        desktop.control().attach(&Agent::new(), false).unwrap();
        assert_eq!(
            view(&desktop)["capabilities"]["terminal_read"]["supported"],
            false
        );
        let terminal = SessionHandle::new("peer".into(), SessionKind::Terminal, Default::default());
        assert!(view(&terminal)["capabilities"]["terminal_read"]["supported"].is_null());
        assert_eq!(
            view(&terminal)["capabilities"]["screen_capture"]["supported"],
            false
        );
    }
}
