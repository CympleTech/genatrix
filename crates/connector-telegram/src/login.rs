//! Signing in, as on a phone: number, code, password if asked.
//!
//! Design: `docs/design/05-connectors.md`, "Telegram：用户账号协议" and
//! "在界面上接入", and design 09's fourth screen. The steps are the same in a
//! terminal and on that screen. A terminal can ask each question and wait;
//! a page asks one question per request, so the sign-in is a [`LoginFlow`]
//! that holds the connection open between the steps, and [`sign_in`] is that
//! flow driven by a [`Prompt`].

use std::sync::Arc;

use grammers_client::client::{LoginToken, PasswordToken};
use grammers_client::{Client, SenderPool, SignInError};

use crate::credentials::Credentials;
use crate::session::{JsonSession, Snapshot};

/// Who signed in.
#[derive(Clone, Debug)]
pub struct SignedIn {
    /// The account's own Telegram user id.
    pub user_id: i64,
    /// The account's display name.
    pub name: String,
    /// The session to keep.
    pub session: Snapshot,
}

/// How the code, and possibly the password, are asked for.
pub trait Prompt {
    /// The code Telegram sent, to the phone or another signed-in device.
    fn code(&self) -> anyhow::Result<String>;
    /// The two-step verification password, with Telegram's hint.
    fn password(&self, hint: Option<&str>) -> anyhow::Result<String>;
}

/// What comes after the code.
#[derive(Debug)]
pub enum Next {
    /// Signed in.
    Done(SignedIn),
    /// Two-step verification is on: the password, with Telegram's hint.
    Password {
        /// What the user wrote for themselves, if anything.
        hint: Option<String>,
    },
}

/// A sign-in in progress: the connection, the session it is filling, and
/// where it has got to. Dropping it abandons the sign-in.
pub struct LoginFlow {
    phone: String,
    client: Client,
    session: Arc<JsonSession>,
    runner: tokio::task::JoinHandle<()>,
    token: LoginToken,
    password: Option<PasswordToken>,
}

impl std::fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFlow")
            .field("phone", &self.phone)
            .field("awaiting_password", &self.password.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for LoginFlow {
    fn drop(&mut self) {
        self.runner.abort();
    }
}

impl LoginFlow {
    /// Ask Telegram to send a code to this number.
    pub async fn start(credentials: &Credentials, phone: &str) -> anyhow::Result<Self> {
        let session = Arc::new(JsonSession::default());
        let pool = SenderPool::new(Arc::clone(&session), credentials.api_id);
        let client = Client::new(pool.handle.clone());
        let runner = tokio::spawn(pool.runner.run());
        let token = match client
            .request_login_code(phone, &credentials.api_hash)
            .await
        {
            Ok(token) => token,
            Err(e) => {
                runner.abort();
                anyhow::bail!("Telegram would not send a code to {phone}: {e}");
            }
        };
        Ok(Self {
            phone: phone.to_owned(),
            client,
            session,
            runner,
            token,
            password: None,
        })
    }

    /// The number being signed in.
    #[must_use]
    pub fn phone(&self) -> &str {
        &self.phone
    }

    /// The code the user was sent. A wrong code can be tried again.
    pub async fn code(&mut self, code: &str) -> anyhow::Result<Next> {
        match self.client.sign_in(&self.token, code.trim()).await {
            Ok(user) => Ok(Next::Done(self.finish(&user)?)),
            Err(SignInError::PasswordRequired(token)) => {
                let hint = token.hint().map(str::to_owned);
                self.password = Some(token);
                Ok(Next::Password { hint })
            }
            Err(SignInError::InvalidCode) => anyhow::bail!("that code was not accepted"),
            Err(SignInError::SignUpRequired) => {
                anyhow::bail!("this number has no Telegram account; Genatrix does not create one")
            }
            Err(e) => anyhow::bail!("sign-in failed: {e:?}"),
        }
    }

    /// The two-step verification password, after [`Next::Password`].
    pub async fn password(&mut self, password: &str) -> anyhow::Result<SignedIn> {
        let token = self
            .password
            .take()
            .ok_or_else(|| anyhow::anyhow!("Telegram has not asked for a password"))?;
        match self
            .client
            .check_password(token, password.trim().as_bytes())
            .await
        {
            Ok(user) => self.finish(&user),
            Err(e) => {
                // The token is spent; ask Telegram for a fresh one so the user
                // can try again without starting over.
                self.password = match self.client.sign_in(&self.token, "").await {
                    Err(SignInError::PasswordRequired(t)) => Some(t),
                    _ => None,
                };
                anyhow::bail!("the password was refused: {e:?}")
            }
        }
    }

    fn finish(&self, user: &grammers_client::peer::User) -> anyhow::Result<SignedIn> {
        Ok(SignedIn {
            user_id: user.id().bot_api_dialog_id_unchecked(),
            name: user.full_name(),
            session: self.session.snapshot()?,
        })
    }
}

/// Sign the phone number in and return the session, asking through `prompt`.
pub async fn sign_in(
    credentials: &Credentials,
    phone: &str,
    prompt: &dyn Prompt,
) -> anyhow::Result<SignedIn> {
    let mut flow = LoginFlow::start(credentials, phone).await?;
    let code = prompt.code()?;
    match flow.code(&code).await? {
        Next::Done(signed_in) => Ok(signed_in),
        Next::Password { hint } => {
            let password = prompt.password(hint.as_deref())?;
            flow.password(&password).await
        }
    }
}
