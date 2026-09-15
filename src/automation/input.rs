use super::{
    control::Permit,
    error::{BridgeError, Result},
    screen::{self, Mapping},
    wire,
};
use hbb_common::{
    message_proto::{ControlKey, KeyEvent, Message, MouseEvent},
    tokio::{
        self,
        sync::{Mutex as AsyncMutex, Semaphore},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

#[derive(Clone, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub snapshot_id: Option<String>,
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
}
impl Button {
    fn code(self) -> i32 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
        }
    }
}
#[derive(Clone, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Move {
        snapshot_id: Option<String>,
        x: i32,
        y: i32,
    },
    ButtonDown {
        button: Button,
        position: Option<Position>,
    },
    ButtonUp {
        button: Button,
        position: Option<Position>,
    },
    Click {
        snapshot_id: Option<String>,
        x: i32,
        y: i32,
        button: Option<Button>,
        count: Option<u8>,
    },
    Drag {
        button: Option<Button>,
        points: Vec<Position>,
        duration_ms: Option<u64>,
    },
    Scroll {
        horizontal: Option<i32>,
        vertical: Option<i32>,
        position: Option<Position>,
    },
    KeyDown {
        key: String,
    },
    KeyUp {
        key: String,
    },
    KeyPress {
        key: String,
    },
    Shortcut {
        modifiers: Vec<Modifier>,
        key: String,
    },
    Text {
        text: String,
    },
    ReleaseAll,
}
#[derive(Clone, Copy, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Modifier {
    Control,
    Shift,
    Alt,
    Meta,
}
impl Modifier {
    fn key(self) -> &'static str {
        match self {
            Self::Control => "ControlLeft",
            Self::Shift => "ShiftLeft",
            Self::Alt => "AltLeft",
            Self::Meta => "MetaLeft",
        }
    }
}
struct Queue {
    capacity: Semaphore,
    gate: AsyncMutex<Held>,
}
#[derive(Default)]
struct Held {
    generation: u64,
    keys: HashSet<String>,
    buttons: HashSet<i32>,
}
fn queue(session: &str) -> Arc<Queue> {
    static QUEUES: OnceLock<Mutex<HashMap<String, Weak<Queue>>>> = OnceLock::new();
    let mut queues = QUEUES.get_or_init(Default::default).lock().unwrap();
    queues.retain(|_, q| q.strong_count() > 0);
    if let Some(q) = queues.get(session).and_then(Weak::upgrade) {
        return q;
    }
    let q = Arc::new(Queue {
        capacity: Semaphore::new(2),
        gate: AsyncMutex::new(Held::default()),
    });
    queues.insert(session.into(), Arc::downgrade(&q));
    q
}
// Held state persists with the wire sender; this separate state protects shortcut semantics.
fn held_states() -> &'static Mutex<HashMap<String, (u64, HashSet<String>, HashSet<i32>)>> {
    static HELD: OnceLock<Mutex<HashMap<String, (u64, HashSet<String>, HashSet<i32>)>>> =
        OnceLock::new();
    HELD.get_or_init(Default::default)
}
#[derive(Serialize)]
pub struct Progress {
    pub delivery: &'static str,
    pub completed_actions: usize,
    pub sent_events: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_action_index: Option<usize>,
    pub held_keys: Vec<String>,
    pub held_buttons: Vec<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}
struct Event {
    message: Message,
    mapping: Option<Mapping>,
    pause: u64,
    release: bool,
}
fn mouse(mask: i32, x: i32, y: i32, mapping: Option<Mapping>) -> Event {
    let mut m = Message::new();
    m.set_mouse_event(MouseEvent {
        mask,
        x,
        y,
        ..Default::default()
    });
    Event {
        message: m,
        mapping,
        pause: 0,
        release: false,
    }
}
fn key_event(name: &str, down: bool) -> Result<Event> {
    let mut key = KeyEvent::new();
    key.down = down;
    if name.len() == 4 && name.starts_with("Key") && name.as_bytes()[3].is_ascii_uppercase() {
        key.set_chr(name.as_bytes()[3].to_ascii_lowercase() as u32);
    } else if name.len() == 6 && name.starts_with("Digit") && name.as_bytes()[5].is_ascii_digit() {
        key.set_chr(name.as_bytes()[5] as u32);
    } else {
        let code = match name {
            "Enter" => ControlKey::Return,
            "Tab" => ControlKey::Tab,
            "Escape" => ControlKey::Escape,
            "Backspace" => ControlKey::Backspace,
            "Delete" => ControlKey::Delete,
            "Insert" => ControlKey::Insert,
            "Space" => ControlKey::Space,
            "ArrowUp" => ControlKey::UpArrow,
            "ArrowDown" => ControlKey::DownArrow,
            "ArrowLeft" => ControlKey::LeftArrow,
            "ArrowRight" => ControlKey::RightArrow,
            "Home" => ControlKey::Home,
            "End" => ControlKey::End,
            "PageUp" => ControlKey::PageUp,
            "PageDown" => ControlKey::PageDown,
            "ControlLeft" => ControlKey::Control,
            "ControlRight" => ControlKey::RControl,
            "ShiftLeft" => ControlKey::Shift,
            "ShiftRight" => ControlKey::RShift,
            "AltLeft" => ControlKey::Alt,
            "AltRight" => ControlKey::RAlt,
            "MetaLeft" => ControlKey::Meta,
            "MetaRight" => ControlKey::RWin,
            "F1" => ControlKey::F1,
            "F2" => ControlKey::F2,
            "F3" => ControlKey::F3,
            "F4" => ControlKey::F4,
            "F5" => ControlKey::F5,
            "F6" => ControlKey::F6,
            "F7" => ControlKey::F7,
            "F8" => ControlKey::F8,
            "F9" => ControlKey::F9,
            "F10" => ControlKey::F10,
            "F11" => ControlKey::F11,
            "F12" => ControlKey::F12,
            _ => return Err(BridgeError::invalid("Unsupported KeyName")),
        };
        key.set_control_key(code);
    }
    let mut message = Message::new();
    message.set_key_event(key);
    Ok(Event {
        message,
        mapping: None,
        pause: 0,
        release: false,
    })
}
fn position(permit: &Permit, p: &Position, default: Option<&str>) -> Result<Event> {
    let id = p
        .snapshot_id
        .as_deref()
        .or(default)
        .ok_or_else(|| BridgeError::invalid("Coordinate actions require snapshot_id"))?;
    let mapping = screen::mapping(permit, id)?;
    let (x, y) = mapping.point(p.x, p.y)?;
    Ok(mouse(0, x, y, Some(mapping)))
}
fn expand(
    permit: &Permit,
    action: &Action,
    default: Option<&str>,
    held: &mut Held,
) -> Result<Vec<Event>> {
    let mut events = vec![];
    match action {
        Action::Move { snapshot_id, x, y } => events.push(position(
            permit,
            &Position {
                snapshot_id: snapshot_id.clone(),
                x: *x,
                y: *y,
            },
            default,
        )?),
        Action::ButtonDown {
            button,
            position: p,
        }
        | Action::ButtonUp {
            button,
            position: p,
        } => {
            if let Some(p) = p {
                events.push(position(permit, p, default)?);
            }
            let down = matches!(action, Action::ButtonDown { .. });
            events.push(mouse(
                (button.code() << 3) | if down { 1 } else { 2 },
                0,
                0,
                None,
            ));
            if down {
                held.buttons.insert(button.code());
            } else {
                held.buttons.remove(&button.code());
            }
        }
        Action::Click {
            snapshot_id,
            x,
            y,
            button,
            count,
        } => {
            let count = count.unwrap_or(1);
            if !(1..=2).contains(&count) {
                return Err(BridgeError::invalid("click count must be 1 or 2"));
            }
            events.push(position(
                permit,
                &Position {
                    snapshot_id: snapshot_id.clone(),
                    x: *x,
                    y: *y,
                },
                default,
            )?);
            let button = button.unwrap_or(Button::Left).code();
            for n in 0..count {
                let mut down = mouse((button << 3) | 1, 0, 0, None);
                if n > 0 {
                    down.pause = 100;
                }
                events.push(down);
                events.push(mouse((button << 3) | 2, 0, 0, None));
            }
            held.buttons.remove(&button);
        }
        Action::Drag {
            button,
            points,
            duration_ms,
        } => {
            let ms = duration_ms.unwrap_or(500);
            if !(2..=64).contains(&points.len()) || !(1..=3000).contains(&ms) {
                return Err(BridgeError::invalid(
                    "drag requires 2..64 points and duration_ms 1..3000",
                ));
            }
            let button = button.unwrap_or(Button::Left).code();
            events.push(position(permit, &points[0], default)?);
            events.push(mouse((button << 3) | 1, 0, 0, None));
            for p in &points[1..] {
                let mut event = position(permit, p, default)?;
                event.pause = ms / (points.len() as u64 - 1);
                events.push(event);
            }
            events.push(mouse((button << 3) | 2, 0, 0, None));
            held.buttons.remove(&button);
        }
        Action::Scroll {
            horizontal,
            vertical,
            position: p,
        } => {
            let (x, y) = (horizontal.unwrap_or(0), vertical.unwrap_or(0));
            if !(-100..=100).contains(&x) || !(-100..=100).contains(&y) || (x == 0 && y == 0) {
                return Err(BridgeError::invalid(
                    "scroll requires nonzero ticks in -100..100",
                ));
            }
            if let Some(p) = p {
                events.push(position(permit, p, default)?);
            }
            events.push(mouse(3, -x, -y, None));
        }
        Action::KeyDown { key } | Action::KeyUp { key } | Action::KeyPress { key } => {
            let down = !matches!(action, Action::KeyUp { .. });
            events.push(key_event(key, down)?);
            if matches!(action, Action::KeyPress { .. }) {
                events.push(key_event(key, false)?);
                held.keys.remove(key);
            } else if down {
                held.keys.insert(key.clone());
            } else {
                held.keys.remove(key);
            }
        }
        Action::Shortcut { modifiers, key } => {
            if modifiers.len() > 4 {
                return Err(BridgeError::invalid("At most four shortcut modifiers"));
            }
            let mut newly = vec![];
            for modifier in modifiers {
                let name = modifier.key();
                if held.keys.insert(name.into()) {
                    events.push(key_event(name, true)?);
                    newly.push(name.to_owned());
                }
            }
            if held.keys.insert(key.clone()) {
                events.push(key_event(key, true)?);
                newly.push(key.clone());
            } else {
                key_event(key, true)?;
            }
            for name in newly.into_iter().rev() {
                events.push(key_event(&name, false)?);
                held.keys.remove(&name);
            }
        }
        Action::Text { text } => {
            if text.len() > 16 * 1024 {
                return Err(BridgeError::invalid("Text exceeds 16 KiB"));
            }
            let mut k = KeyEvent::new();
            k.set_seq(text.clone());
            let mut m = Message::new();
            m.set_key_event(k);
            events.push(Event {
                message: m,
                mapping: None,
                pause: 0,
                release: false,
            });
        }
        Action::ReleaseAll => {
            events.push(Event {
                message: Message::new(),
                mapping: None,
                pause: 0,
                release: true,
            });
            held.keys.clear();
            held.buttons.clear();
        }
    }
    let mut mapping = None;
    for event in &mut events {
        if event.mapping.is_some() {
            mapping = event.mapping.clone();
        } else if matches!(
            event.message.union,
            Some(hbb_common::message_proto::message::Union::MouseEvent(_))
        ) {
            event.mapping = mapping.clone();
        }
    }
    Ok(events)
}
pub fn validate(permit: &Permit, actions: &[Action], snapshot: Option<&str>) -> Result<()> {
    if !(1..=32).contains(&actions.len()) {
        return Err(BridgeError::invalid("Input requires 1..32 actions"));
    }
    let mut held = Held::default();
    let mut duration = 0;
    for action in actions {
        duration += expand(permit, action, snapshot, &mut held)?
            .iter()
            .map(|e| e.pause)
            .sum::<u64>();
    }
    if duration > 5000 {
        return Err(BridgeError::invalid("Input batch exceeds five seconds"));
    }
    Ok(())
}
async fn revoked(permit: &Permit) -> BridgeError {
    let mut changed = permit.authority.subscribe();
    loop {
        if let Err(error) = permit.check() {
            return error;
        }
        if changed.changed().await.is_err() {
            return BridgeError::new("SESSION_CLOSED", "Input authority is closed");
        }
    }
}
pub async fn send(
    permit: Permit,
    actions: Vec<Action>,
    snapshot: Option<String>,
    cancel: impl std::future::Future<Output = ()> + Send,
) -> Result<Progress> {
    let mut cancel = Box::pin(cancel);
    let platform = super::sessions::get(&permit.authority.session_id)
        .and_then(|s| s.snapshot().platform)
        .ok_or_else(|| BridgeError::new("NOT_READY", "Remote platform has not been reported"))?;
    permit.check()?;
    validate(&permit, &actions, snapshot.as_deref())?;
    let queue = queue(&permit.authority.session_id);
    let _capacity = queue.capacity.try_acquire().map_err(|_| {
        BridgeError::new(
            "INPUT_BUSY",
            "One batch is executing and one is already queued",
        )
    })?;
    let mut held = tokio::select! { biased; _=&mut cancel => return Err(BridgeError::new("CANCELLED","Input batch cancelled before execution")), error=revoked(&permit)=>return Err(error), held=queue.gate.lock()=>held };
    permit.check()?;
    if held.generation != permit.generation {
        *held = Held::default();
    }
    if let Some((generation, keys, buttons)) = held_states()
        .lock()
        .unwrap()
        .get(&permit.authority.session_id)
        .cloned()
    {
        if generation == permit.generation {
            held.keys = keys;
            held.buttons = buttons;
        }
    }
    held.generation = permit.generation;
    struct Cleanup(Permit, bool);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.1 {
                wire::cleanup(&self.0);
                held_states()
                    .lock()
                    .unwrap()
                    .remove(&self.0.authority.session_id);
            }
        }
    }
    let mut cleanup = Cleanup(permit.clone(), true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut progress = Progress {
        delivery: "not_sent",
        completed_actions: 0,
        sent_events: 0,
        failed_action_index: None,
        held_keys: vec![],
        held_buttons: vec![],
        error: None,
    };
    for (index, action) in actions.iter().enumerate() {
        let events = expand(&permit, action, snapshot.as_deref(), &mut held);
        let result = async {
            for mut event in events? {
                physical_character(&mut event.message, &platform)?;
                if event.pause > 0 {
                    tokio::select! {
                        biased;
                        error = revoked(&permit) => return Err(error),
                        _ = tokio::time::sleep(Duration::from_millis(event.pause)) => {},
                    }
                }
                permit.check()?;
                progress.sent_events +=
                    wire::send_event(permit.clone(), event.message, event.mapping, event.release)
                        .await?;
                progress.delivery = "sent";
            }
            Ok::<_, BridgeError>(())
        };
        let result = tokio::select! { biased;
            _ = &mut cancel => Err(BridgeError::new("CANCELLED", "Input batch was cancelled; an in-flight send may have reached the remote")),
            result=tokio::time::timeout_at(deadline,result) => result.unwrap_or_else(|_|Err(BridgeError::new("DELIVERY_UNKNOWN","Input batch exceeded its deadline"))),
        };
        if let Err(error) = result {
            if matches!(error.code, "DELIVERY_UNKNOWN" | "CANCELLED") {
                progress.delivery = "unknown";
            }
            progress.failed_action_index = Some(index);
            progress.error = Some(error);
            break;
        }
        progress.completed_actions += 1;
    }
    if progress.error.is_none() {
        cleanup.1 = false;
        held_states().lock().unwrap().insert(
            permit.authority.session_id.clone(),
            (permit.generation, held.keys.clone(), held.buttons.clone()),
        );
        progress.held_keys = held.keys.iter().cloned().collect();
        progress.held_buttons = held.buttons.iter().copied().collect();
    }
    if progress.error.is_some() && permit.check().is_ok() {
        let cleanup_result = tokio::time::timeout(
            Duration::from_secs(1),
            wire::send_event(permit.clone(), Message::new(), None, true),
        )
        .await;
        if matches!(cleanup_result, Ok(Ok(_))) {
            cleanup.1 = false;
            held_states()
                .lock()
                .unwrap()
                .remove(&permit.authority.session_id);
        } else if let Some(error) = &mut progress.error {
            error.details = Some(serde_json::json!({"input_cleanup":"unconfirmed"}));
        }
    }
    Ok(progress)
}
pub fn forget(session: &str) {
    held_states().lock().unwrap().remove(session);
}

fn physical_character(message: &mut Message, platform: &str) -> Result<()> {
    use hbb_common::message_proto::{key_event, message as proto, KeyboardMode};
    use rdev::Key;
    let Some(proto::Union::KeyEvent(event)) = &mut message.union else {
        return Ok(());
    };
    let Some(key_event::Union::Chr(chr)) = event.union else {
        return Ok(());
    };
    const LETTERS: [Key; 26] = [
        Key::KeyA,
        Key::KeyB,
        Key::KeyC,
        Key::KeyD,
        Key::KeyE,
        Key::KeyF,
        Key::KeyG,
        Key::KeyH,
        Key::KeyI,
        Key::KeyJ,
        Key::KeyK,
        Key::KeyL,
        Key::KeyM,
        Key::KeyN,
        Key::KeyO,
        Key::KeyP,
        Key::KeyQ,
        Key::KeyR,
        Key::KeyS,
        Key::KeyT,
        Key::KeyU,
        Key::KeyV,
        Key::KeyW,
        Key::KeyX,
        Key::KeyY,
        Key::KeyZ,
    ];
    const DIGITS: [Key; 10] = [
        Key::Num0,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    let key = if (b'a' as u32..=b'z' as u32).contains(&chr) {
        LETTERS[(chr - b'a' as u32) as usize]
    } else if (b'0' as u32..=b'9' as u32).contains(&chr) {
        DIGITS[(chr - b'0' as u32) as usize]
    } else {
        return Err(BridgeError::invalid("Unsupported physical key"));
    };
    let code = match platform {
        "Windows" => rdev::win_scancode_from_key(key),
        "Mac OS" => rdev::macos_keycode_from_key(key).map(u32::from),
        "Linux" => rdev::linux_keycode_from_key(key),
        _ => None,
    }
    .ok_or_else(|| {
        BridgeError::new(
            "UNSUPPORTED",
            "Physical letter/digit keys are not mapped for this remote platform; use text",
        )
    })?;
    event.set_chr(code);
    event.mode = KeyboardMode::Map.into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn takeover_wakes_input_waiters_without_waiting_for_the_drag_delay() {
        let authority = Arc::new(super::super::control::Authority::new(
            "input-wait-test".into(),
        ));
        let agent = super::super::control::Agent::new();
        authority.attach(&agent, true).unwrap();
        if let Some((generation, _)) = authority.pending() {
            authority.complete_transition(generation, None);
        }
        let permit = authority
            .resolve(&agent, &authority.view().session_ref.unwrap(), true)
            .unwrap();
        let waiting = tokio::spawn(async move { revoked(&permit).await });
        tokio::task::yield_now().await;
        authority.release(None, false).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
                .code,
            "CONTROL_EXPIRED"
        );
    }
    #[test]
    fn letters_use_remote_physical_codes_so_shift_is_not_discarded() {
        let mut event = key_event("KeyA", true).unwrap();
        physical_character(&mut event.message, "Windows").unwrap();
        let key = event.message.key_event();
        assert_eq!(key.chr(), 0x1e);
        assert_eq!(
            key.mode.enum_value().unwrap(),
            hbb_common::message_proto::KeyboardMode::Map
        );
        assert!(key.down);
    }
    #[test]
    fn all_supported_letters_digits_and_control_keys_have_valid_messages() {
        for c in b'A'..=b'Z' {
            for platform in ["Windows", "Linux", "Mac OS"] {
                let mut event = key_event(&format!("Key{}", c as char), true).unwrap();
                physical_character(&mut event.message, platform).unwrap();
            }
        }
        for c in b'0'..=b'9' {
            let mut event = key_event(&format!("Digit{}", c as char), false).unwrap();
            physical_character(&mut event.message, "Windows").unwrap();
        }
        for key in ["Enter", "ControlRight", "ShiftLeft", "MetaRight", "F12"] {
            key_event(key, true).unwrap();
        }
        assert!(key_event("UnsupportedKey", true).is_err());
    }
}
