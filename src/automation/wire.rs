use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions,
};
use crate::{
    client::Data,
    ui_session_interface::{InvokeUiSession, Session},
};
use hbb_common::{
    message_proto::{key_event, message, Message},
    tokio::sync::oneshot,
    Stream,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone)]
pub struct Envelope {
    pub permit: Permit,
    pub message: Message,
    pub mapping: Option<super::screen::Mapping>,
    pub release: bool,
    pub reply: Arc<Mutex<Option<oneshot::Sender<Result<usize>>>>>,
    pub active: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
pub struct WireState {
    held: HashMap<String, Message>,
}

pub fn is_input(message: &Message) -> bool {
    matches!(
        message.union,
        Some(
            message::Union::KeyEvent(_)
                | message::Union::MouseEvent(_)
                | message::Union::TerminalAction(_)
                | message::Union::PointerDeviceEvent(_)
        )
    )
}

pub fn wrap_gui<T: InvokeUiSession>(core: &Session<T>, data: Data) -> Option<Data> {
    let Some(session) = sessions::for_core(core) else {
        return Some(data);
    };
    if let Data::Message(message) = &data {
        super::subscriptions::observe_gui(core, message);
    }
    match data {
        Data::Message(message) if is_input(&message) => {
            let Some(permit) = session.control().human_permit() else {
                return Some(Data::Message(message));
            };
            if permit.check().is_err() {
                return None;
            }
            Some(Data::Automation(Envelope {
                permit,
                message,
                mapping: None,
                release: false,
                reply: Default::default(),
                active: None,
            }))
        }
        Data::Login((username, os_password, password, remember))
            if session.control().installed() =>
        {
            let permit = session.control().human_permit()?;
            if permit.check().is_err() {
                return None;
            }
            Some(Data::AutomationLogin(super::auth::Envelope {
                permit,
                challenge: None,
                credentials: super::auth::Credentials::Human(
                    username,
                    os_password,
                    password,
                    remember,
                ),
                active: Arc::new(AtomicBool::new(true)),
                reply: Default::default(),
            }))
        }
        Data::Message(message)
            if matches!(message.union, Some(message::Union::Auth2fa(_)))
                && session.control().installed() =>
        {
            let permit = session.control().human_permit()?;
            if permit.check().is_err() {
                return None;
            }
            if let Some(message::Union::Auth2fa(auth)) = message.union {
                Some(Data::AutomationLogin(super::auth::Envelope {
                    permit,
                    challenge: None,
                    credentials: super::auth::Credentials::HumanTwoFactor(auth),
                    active: Arc::new(AtomicBool::new(true)),
                    reply: Default::default(),
                }))
            } else {
                None
            }
        }
        other => Some(other),
    }
}

pub fn reject_unmarked<T: InvokeUiSession>(core: &Session<T>, message: &Message) -> bool {
    (is_input(message) || matches!(message.union, Some(message::Union::Auth2fa(_))))
        && sessions::for_core(core).is_some_and(|session| session.control().rejects_unmarked())
}

impl WireState {
    pub fn observe_manual(&mut self, message: &Message) {
        self.track(message);
    }
    fn track(&mut self, message: &Message) {
        // These protocol commands execute even on key-up; never synthesize a release.
        if let Some(message::Union::KeyEvent(key)) = &message.union {
            if matches!(&key.union, Some(key_event::Union::ControlKey(k)) if matches!(k.enum_value(), Ok(hbb_common::message_proto::ControlKey::LockScreen | hbb_common::message_proto::ControlKey::CtrlAltDel))) { return; }
        }
        match &message.union {
            Some(message::Union::KeyEvent(key))
                if !matches!(key.union, Some(key_event::Union::Seq(_))) =>
            {
                let id = format!("key:{:?}", key.union);
                if key.down {
                    let mut release = key.clone();
                    release.down = false;
                    release.press = false;
                    let mut message = Message::new();
                    message.set_key_event(release);
                    self.held.insert(id, message);
                } else {
                    self.held.remove(&id);
                }
            }
            Some(message::Union::MouseEvent(mouse)) => {
                let kind = mouse.mask & 7;
                let id = format!("button:{}", mouse.mask >> 3);
                if kind == 1 {
                    let mut release = mouse.clone();
                    release.mask = (mouse.mask & !7) | 2;
                    let mut message = Message::new();
                    message.set_mouse_event(release);
                    self.held.insert(id, message);
                } else if kind == 2 {
                    self.held.remove(&id);
                }
            }
            _ => {}
        }
    }

    pub async fn release_all(&mut self, peer: &mut Stream) -> Option<String> {
        let mut error = None;
        for (_, message) in self.held.drain() {
            if peer.send(&message).await.is_err() {
                error = Some(
                    "Some held inputs could not be released on the remote connection".to_owned(),
                );
            }
        }
        error
    }

    pub async fn transition<T: InvokeUiSession>(&mut self, core: &Session<T>, peer: &mut Stream) {
        super::files::reap(core);
        let Some(session) = sessions::for_core(core) else {
            return;
        };
        let control = session.control();
        if let Some((generation, _)) = control.pending() {
            let keys = self.held.keys().filter(|id| id.starts_with("key:")).count();
            let buttons = self.held.len() - keys;
            let error = self.release_all(peer).await;
            control.record_released(generation, keys, buttons);
            super::input::forget(&control.session_id);
            control.complete_transition(generation, error);
        }
    }

    pub async fn send(&mut self, mut envelope: Envelope, peer: &mut Stream) {
        let result = envelope.permit.check().and_then(|_| {
            if envelope
                .active
                .as_ref()
                .is_some_and(|active| !active.load(Ordering::Acquire))
            {
                return Err(BridgeError::new(
                    "CANCELLED",
                    "Input call was cancelled before sending",
                ));
            }
            if let Some(mapping) = &envelope.mapping {
                mapping.check(&envelope.permit)?;
            }
            let session = sessions::get(&envelope.permit.authority.session_id)
                .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "GUI session is closed"))?;
            let snapshot = session.snapshot();
            if snapshot.connection_epoch != envelope.permit.epoch {
                return Err(BridgeError::new(
                    "CONTROL_EXPIRED",
                    "Remote connection changed",
                ));
            }
            if !envelope.permit.human && !envelope.release {
                if !snapshot.authenticated || snapshot.state != sessions::ConnectionState::Ready {
                    return Err(BridgeError::new(
                        "NOT_READY",
                        "Remote session is not ready for input",
                    ));
                }
                if matches!(envelope.message.union, Some(message::Union::Cliprdr(_))) { super::file_clipboard::check(&envelope.permit, true)?; }
                if let Some(message::Union::Misc(misc)) = &envelope.message.union {
                    if matches!(misc.union, Some(hbb_common::message_proto::misc::Union::RestartRemoteDevice(_))) {
                        super::desktop::restart_check(&envelope.permit)?;
                    }
                    if matches!(misc.union, Some(hbb_common::message_proto::misc::Union::ChangeDisplayResolution(_) | hbb_common::message_proto::misc::Union::ChangeResolution(_) | hbb_common::message_proto::misc::Union::ToggleVirtualDisplay(_))) {
                        super::displays::remote_write_check(&envelope.permit)?;
                    }
                    if let Some(hbb_common::message_proto::misc::Union::Option(option)) = &misc.union {
                        if option.enable_file_transfer.value() != 0 { super::file_clipboard::check(&envelope.permit, false)?; }
                    }
                }
                if matches!(envelope.message.union, Some(message::Union::Clipboard(_) | message::Union::MultiClipboards(_))) {
                    super::text_clipboard::check(&envelope.permit, true, true)?;
                }
                if matches!(
                    envelope.message.union,
                    Some(message::Union::KeyEvent(_) | message::Union::MouseEvent(_))
                ) && snapshot.permissions.get("keyboard") != Some(&true)
                {
                    return Err(BridgeError::new(
                        "PERMISSION_DENIED",
                        "Remote keyboard/mouse permission is not granted",
                    ));
                }
                if matches!(envelope.message.union, Some(message::Union::KeyEvent(_) | message::Union::MouseEvent(_)))
                    && sessions::core(&snapshot.session_id).is_some_and(|core| core.lc.read().unwrap().view_only.v)
                {
                    return Err(BridgeError::new("VIEW_ONLY", "Keyboard and mouse input are disabled in view-only mode"));
                }
            }
            if !envelope.permit.human {
                if let Some(message::Union::TerminalAction(action)) = &envelope.message.union {
                    super::terminals::check_action(&envelope.permit, action)?;
                }
            }
            Ok(())
        });
        let result = match result {
            Err(error) => Err(error),
            Ok(()) if envelope.release => {
                let count = self.held.len();
                self.release_all(peer).await.map_or(Ok(count), |error| {
                    Err(BridgeError::new("DELIVERY_UNKNOWN", error))
                })
            }
            Ok(()) => {
                if !envelope.permit.human {
                    let platform = sessions::get(&envelope.permit.authority.session_id)
                        .and_then(|session| session.snapshot().platform).unwrap_or_default();
                    let modifiers = self
                        .held
                        .values()
                        .filter_map(|m| {
                            let Some(message::Union::KeyEvent(key)) = &m.union else {
                                return None;
                            };
                            super::input::modifier(key, &platform).map(Into::into)
                        })
                        .collect::<Vec<_>>();
                    match &mut envelope.message.union {
                        Some(message::Union::KeyEvent(key)) => key.modifiers = modifiers,
                        Some(message::Union::MouseEvent(mouse)) => mouse.modifiers = modifiers,
                        _ => {}
                    }
                }
                self.track(&envelope.message);
                peer.send(&envelope.message).await.map(|_| 1).map_err(|_| {
                    BridgeError::new("DELIVERY_UNKNOWN", "Remote transport failed during send")
                })
            }
        };
        if let Err(error) = &result {
            if let Some(message::Union::TerminalAction(action)) = &envelope.message.union {
                if let Some(hbb_common::message_proto::terminal_action::Union::Open(open)) =
                    &action.union
                {
                    super::terminals::fail(
                        &envelope.permit.authority.session_id,
                        open.terminal_id,
                        &error.message,
                    );
                }
            }
        }
        if let Some(reply) = envelope.reply.lock().unwrap().take() {
            // A cancelled caller deliberately drops the receiver; it does not undo sent bytes.
            let _cancelled_receiver = reply.send(result);
        }
    }
}

pub async fn send(permit: Permit, message: Message) -> Result<()> {
    send_event(permit, message, None, false).await.map(|_| ())
}
pub async fn send_event(
    permit: Permit,
    message: Message,
    mapping: Option<super::screen::Mapping>,
    release: bool,
) -> Result<usize> {
    permit.check()?;
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "GUI session is closed"))?;
    let sender = core
        .sender
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| BridgeError::new("DISCONNECTED", "Remote sender is unavailable"))?;
    let (reply, response) = oneshot::channel();
    struct Active(Arc<AtomicBool>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let active = Active(Arc::new(AtomicBool::new(true)));
    sender
        .send(Data::Automation(Envelope {
            permit,
            message,
            mapping,
            release,
            reply: Arc::new(Mutex::new(Some(reply))),
            active: Some(active.0.clone()),
        }))
        .map_err(|_| BridgeError::new("DISCONNECTED", "Remote sender is closed"))?;
    response.await.map_err(|_| {
        BridgeError::new(
            "DELIVERY_UNKNOWN",
            "Remote sender ended without a delivery result",
        )
    })?
}

pub fn wake(session_id: &str) {
    if let Some(core) = sessions::core(session_id) {
        if let Some(sender) = core.sender.read().unwrap().as_ref() {
            let _closed_sender = sender.send(Data::AutomationWake);
        }
    }
}

pub fn cleanup(permit: &Permit) {
    if let Some(core) = sessions::core(&permit.authority.session_id) {
        if let Some(sender) = core.sender.read().unwrap().as_ref() {
            let _closed_sender = sender.send(Data::Automation(Envelope {
                permit: permit.clone(),
                message: Message::new(),
                mapping: None,
                release: true,
                reply: Default::default(),
                active: None,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::control::Agent;
    use hbb_common::{
        message_proto::{ControlKey, KeyEvent},
        tokio::{self, io::AsyncReadExt},
    };
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn one_shot_system_keys_do_not_produce_release_commands() {
        let mut state = WireState::default();
        for key in [crate::keyboard::client::event_lock_screen(),crate::keyboard::client::event_ctrl_alt_del()] {
            let mut message=Message::new();message.set_key_event(key);
            state.track(&message);
        }
        assert!(state.held.is_empty());
    }

    #[tokio::test]
    async fn handover_releases_held_input_and_rejects_previously_queued_events() {
        let core = Session::<crate::flutter::FlutterHandler>::default();
        let view = Uuid::new_v4();
        sessions::add_view(&core, &view);
        let session = sessions::for_core(&core).unwrap();
        let control = session.control();
        let agent = Agent::new();
        control.attach(&agent, true).unwrap();
        control.complete_transition(control.pending().unwrap().0, None);
        let permit = control
            .resolve(&agent, &control.view().session_ref.unwrap(), true)
            .unwrap();
        let mut key = KeyEvent::new();
        key.down = true;
        key.set_control_key(ControlKey::Shift);
        let mut message = Message::new();
        message.set_key_event(key);
        let mut wire = WireState::default();
        wire.observe_manual(&message);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (local, accepted) = tokio::join!(tokio::net::TcpStream::connect(address), listener.accept());
        let (mut remote, _) = accepted.unwrap();
        let mut stream = Stream::from(local.unwrap(), address);
        control.release(None, false).unwrap();
        wire.transition(&core, &mut stream).await;
        let mut bytes = [0; 4096];
        assert!(
            tokio::time::timeout(Duration::from_secs(1), remote.read(&mut bytes))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        assert!(wire.held.is_empty());
        assert_eq!(control.view().released_inputs.unwrap().keys, 1);
        let (reply, response) = oneshot::channel();
        wire.send(
            Envelope {
                permit,
                message,
                mapping: None,
                release: false,
                reply: Arc::new(Mutex::new(Some(reply))),
                active: None,
            },
            &mut stream,
        )
        .await;
        assert!(response.await.unwrap().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), remote.read(&mut bytes))
                .await
                .is_err()
        );
        sessions::remove_view(&core, &view);
    }
}
