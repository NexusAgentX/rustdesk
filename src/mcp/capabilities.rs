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
    let mut result = json!({
        "session_ref":c.session_ref,
        "peer":{"platform":s.platform,"version":s.peer_version,"authenticated":s.authenticated,
            "connection_state":crate::automation::api::state_name(s.state),"permissions":s.permissions},
        "capabilities":{
            "session_read":cap(Some(true),Some(true),false,false,&["rd_session_get","rd_capabilities_get"]),
            "session_lifecycle":cap(Some(true),Some(true),true,false,&["rd_session_disconnect","rd_session_reconnect","rd_session_close"]),
            "screen_capture":cap(Some(desktop),Some(true),false,true,&["rd_screen_capture"]),
            "file_read":cap(Some(s.kind == SessionKind::FileTransfer),s.permissions.get("file").copied(),false,true,&["rd_file_list","rd_file_jobs","rd_file_job_get"]),
            "file_write":cap(Some(s.kind == SessionKind::FileTransfer),s.permissions.get("file").copied(),true,true,&["rd_file_transfer","rd_file_job_cancel","rd_file_conflict_resolve"]),
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
            "settings":{"write_mode":"explicit_value","scope_required":true,"scopes":["session","peer_preference","global","local_window"]},
            "credentials":"Never included in operation arguments or capability results; authentication values are single-use.",
            "future_features":"Capabilities list only implemented MCP operations. GUI-only features are not promises of MCP support."
        }
    });
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
            if lc.view_only.v {
                for key in ["keyboard_mouse", "clipboard_settings_write"] {
                    result["capabilities"][key]["available"] = json!(false);
                    if let Some(blockers) = result["capabilities"][key]["blockers"].as_array_mut() {
                        blockers.push(json!("view_only"));
                    }
                }
            }
        }
    }
    for key in ["clipboard_settings_read", "clipboard_settings_write"] {
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
