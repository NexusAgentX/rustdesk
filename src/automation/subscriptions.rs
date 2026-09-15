use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions,
};
use crate::{
    client::Data,
    ui_session_interface::{InvokeUiSession, Session},
};
use hbb_common::message_proto::{message, misc, CaptureDisplays, Message, Misc};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Mutex, OnceLock},
};
#[derive(Default)]
struct Desired {
    gui: BTreeSet<i32>,
    ai: BTreeSet<i32>,
    binding: Option<String>,
    managed: bool,
}
fn states() -> &'static Mutex<HashMap<String, Desired>> {
    static STATES: OnceLock<Mutex<HashMap<String, Desired>>> = OnceLock::new();
    STATES.get_or_init(Default::default)
}
fn capture(message: &Message) -> Option<&CaptureDisplays> {
    if let Some(message::Union::Misc(m)) = &message.union {
        if let Some(misc::Union::CaptureDisplays(c)) = &m.union {
            return Some(c);
        }
    }
    None
}
pub fn observe_gui<T: InvokeUiSession>(core: &Session<T>, message: &Message) {
    let Some(c) = capture(message) else {
        return;
    };
    let Some(session) = sessions::for_core(core) else {
        return;
    };
    let id = session.snapshot().session_id;
    let mut states = states().lock().unwrap();
    let desired = states.entry(id).or_default();
    if !c.set.is_empty() {
        desired.gui = c.set.iter().copied().collect();
    }
    desired.gui.extend(&c.add);
    for id in &c.sub {
        desired.gui.remove(id);
    }
}
fn message(displays: Vec<i32>) -> Message {
    let mut misc = Misc::new();
    misc.set_capture_displays(CaptureDisplays {
        set: displays,
        ..Default::default()
    });
    let mut message = Message::new();
    message.set_misc(misc);
    message
}
pub fn at_send<T: InvokeUiSession>(core: &Session<T>, original: Message) -> Message {
    if capture(&original).is_none() {
        return original;
    }
    let Some(session) = sessions::for_core(core) else {
        return original;
    };
    let id = session.snapshot().session_id;
    let states = states().lock().unwrap();
    let Some(desired) = states.get(&id) else {
        return original;
    };
    if !desired.managed {
        return original;
    }
    message(desired.gui.union(&desired.ai).copied().collect())
}
pub fn need(permit: &Permit, display: i32) -> Result<()> {
    permit.read_check()?;
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
    let snapshot = session.snapshot();
    let displays = {
        let mut states = states().lock().unwrap();
        let desired = states
            .entry(snapshot.session_id.clone())
            .or_insert_with(|| Desired {
                gui: std::iter::once(snapshot.current_display as i32).collect(),
                ..Default::default()
            });
        if desired.binding.as_deref() != Some(permit.binding_id()) {
            desired.ai.clear();
            desired.binding = Some(permit.binding_id().into());
        }
        desired.ai.retain(|id| {
            snapshot
                .displays
                .iter()
                .any(|d| d.id == *id as usize && d.online)
        });
        desired.managed = true;
        desired.ai.insert(display);
        desired.gui.union(&desired.ai).copied().collect()
    };
    if let Some(core) = sessions::core(&snapshot.session_id) {
        let sender = core
            .sender
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| BridgeError::new("DISCONNECTED", "Video sender is unavailable"))?;
        sender
            .send(Data::Message(message(displays)))
            .map_err(|_| BridgeError::new("DISCONNECTED", "Video sender closed"))?;
    }
    Ok(())
}
pub fn detach(session: &str) {
    let displays = {
        let mut states = states().lock().unwrap();
        let Some(desired) = states.get_mut(session) else {
            return;
        };
        desired.ai.clear();
        desired.binding = None;
        desired.gui.iter().copied().collect::<Vec<_>>()
    };
    if !displays.is_empty() {
        if let Some(core) = sessions::core(session) {
            if let Some(sender) = core.sender.read().unwrap().as_ref() {
                let _closed_sender = sender.send(Data::Message(message(displays)));
            }
        }
    }
}
pub fn forget(session: &str) {
    states().lock().unwrap().remove(session);
}
