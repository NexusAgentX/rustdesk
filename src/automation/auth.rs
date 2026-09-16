use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions,
};
use crate::{
    client::{Data, Interface},
    ui_session_interface::{InvokeUiSession, Session},
};
use hbb_common::{message_proto::Message, tokio::sync::oneshot, Stream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone)]
pub enum Credentials {
    Password(String),
    TwoFactor(String),
    HumanTwoFactor(hbb_common::message_proto::Auth2FA),
    OsLogin(String, String, Option<String>),
    Human(String, String, String, bool),
}
impl Credentials {
    fn kind(&self) -> &'static str {
        match self {
            Self::Password(_) => "password",
            Self::TwoFactor(_) => "two_factor",
            Self::OsLogin(..) => "os_login",
            Self::Human(..) | Self::HumanTwoFactor(_) => "human",
        }
    }
    fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Password(p) => !p.is_empty() && p.len() <= 16384,
            Self::TwoFactor(c) => !c.is_empty() && c.len() <= 256,
            Self::OsLogin(u, p, connection) => !u.is_empty() && u.len() <= 256 && p.len() <= 16384 && connection.as_ref().is_none_or(|p| !p.is_empty() && p.len() <= 16384),
            Self::Human(..) | Self::HumanTwoFactor(_) => true,
        };
        if valid {
            Ok(())
        } else {
            Err(BridgeError::invalid("Credentials exceed their limits"))
        }
    }
}
#[derive(Clone)]
pub struct Envelope {
    pub permit: Permit,
    pub challenge: Option<String>,
    pub credentials: Credentials,
    pub active: Arc<AtomicBool>,
    pub reply: Arc<Mutex<Option<oneshot::Sender<Result<()>>>>>,
}
impl Envelope {
    pub fn check(&self) -> Result<()> {
        self.permit.check()?;
        if !self.active.load(Ordering::Acquire) {
            return Err(BridgeError::new(
                "CANCELLED",
                "Authentication was cancelled",
            ));
        }
        self.credentials.validate()?;
        if self.permit.human {
            return Ok(());
        }
        let session = sessions::get(&self.permit.authority.session_id)
            .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
        let state = session.snapshot();
        let valid = state.auth_challenge.as_ref().is_some_and(|c| {
            Some(&c.id) == self.challenge.as_ref() && c.kind == self.credentials.kind()
        });
        if state.auth_challenge.as_ref().is_some_and(|c| c.fields.iter().any(|f| f=="connection_password"))
            && matches!(&self.credentials, Credentials::OsLogin(_,_,None)) {
            return Err(BridgeError::invalid("This OS-login challenge also requires connection_password (the RustDesk password)"));
        }
        if !valid {
            return Err(BridgeError::new(
                "AUTH_CHALLENGE_CHANGED",
                "Read the current authentication challenge before submitting",
            ));
        }
        Ok(())
    }
    pub fn complete(&self, result: Result<()>) {
        if let Some(reply) = self.reply.lock().unwrap().take() {
            let _cancelled_receiver = reply.send(result);
        }
    }
}
pub async fn submit(permit: Permit, challenge: String, credentials: Credentials) -> Result<()> {
    let (reply, response) = oneshot::channel();
    let envelope = Envelope {
        permit,
        challenge: Some(challenge),
        credentials,
        active: Arc::new(AtomicBool::new(true)),
        reply: Arc::new(Mutex::new(Some(reply))),
    };
    envelope.check()?;
    let core = sessions::core(&envelope.permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Visible session has not registered"))?;
    struct Active(Arc<AtomicBool>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _active = Active(envelope.active.clone());
    let sender = core
        .sender
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| BridgeError::new("DISCONNECTED", "Remote sender is unavailable"))?;
    sender
        .send(Data::AutomationLogin(envelope))
        .map_err(|_| BridgeError::new("DISCONNECTED", "Remote sender closed"))?;
    response.await.map_err(|_| {
        BridgeError::new(
            "DELIVERY_UNKNOWN",
            "Authentication sender ended without confirmation",
        )
    })?
}

pub async fn send_login(
    lc: &Arc<std::sync::RwLock<crate::client::LoginConfigHandler>>,
    peer: &mut hbb_common::Stream,
    message: &hbb_common::message_proto::Message,
) -> bool {
    let permit = lc.read().unwrap().automation_auth_permit.clone();
    let Some(permit) = permit else {
        return false;
    };
    let result = match permit.check() {
        Ok(()) => peer
            .send(message)
            .await
            .map_err(|_| BridgeError::new("DELIVERY_UNKNOWN", "Authentication transport failed")),
        Err(error) => {
            lc.write().unwrap().automation_forget_credentials();
            if let Some(core) = sessions::core(&permit.authority.session_id) {
                core.msgbox("input-password", "Password Required", "", "");
            }
            Err(error)
        }
    };
    lc.write().unwrap().automation_auth_result = Some(result);
    true
}

pub async fn handle<T: InvokeUiSession>(core: &Session<T>, envelope: Envelope, peer: &mut Stream) {
    let mut result = envelope.check();
    if result.is_ok() {
        if envelope.permit.human {
            core.lc.write().unwrap().automation_forget_credentials();
        }
        core.lc
            .write()
            .unwrap()
            .automation_authentication(Some(envelope.permit.clone()), false);
        match envelope.credentials.clone() {
            Credentials::HumanTwoFactor(auth) => {
                let mut message = Message::new();
                message.set_auth_2fa(auth);
                envelope.complete(peer.send(&message).await.map_err(|_| {
                    crate::automation::error::BridgeError::new(
                        "DELIVERY_UNKNOWN",
                        "Authentication transport failed",
                    )
                }));
                return;
            }
            Credentials::TwoFactor(code) => {
                let mut message = Message::new();
                message.set_auth_2fa(hbb_common::message_proto::Auth2FA {
                    code,
                    ..Default::default()
                });
                let result = peer.send(&message).await.map_err(|_| {
                    crate::automation::error::BridgeError::new(
                        "DELIVERY_UNKNOWN",
                        "Authentication transport failed",
                    )
                });
                envelope.complete(result);
                return;
            }
            credentials => {
                let (user, os_password, password, remember) = match credentials {
                    Credentials::Password(password) => {
                        (String::new(), String::new(), password, false)
                    }
                    Credentials::OsLogin(user, password, connection_password) => (user, password, connection_password.unwrap_or_default(), false),
                    Credentials::Human(user, os_password, password, remember) => {
                        (user, os_password, password, remember)
                    }
                    Credentials::TwoFactor(_) | Credentials::HumanTwoFactor(_) => unreachable!(),
                };
                core.handle_login_from_ui(user, os_password, password, remember, peer)
                    .await;
                result = core
                    .lc
                    .write()
                    .unwrap()
                    .automation_auth_result
                    .take()
                    .unwrap_or_else(|| {
                        Err(crate::automation::error::BridgeError::new(
                            "DELIVERY_UNKNOWN",
                            "Authentication completed without delivery confirmation",
                        ))
                    });
            }
        }
    }
    envelope.complete(result.and_then(|_| envelope.permit.check()));
}
