//! Signing in, as on a phone: number, code, password if asked.
//!
//! Design: `docs/design/05-connectors.md`, "Telegram：用户账号协议", and
//! design 09's fourth screen. The steps are the same in a terminal and on
//! that screen; what differs is only how the code is asked for, so the
//! asking is a callback.

use std::sync::Arc;

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

/// Sign the phone number in and return the session.
pub async fn sign_in(
    credentials: &Credentials,
    phone: &str,
    prompt: &dyn Prompt,
) -> anyhow::Result<SignedIn> {
    let session = Arc::new(JsonSession::default());
    let pool = SenderPool::new(Arc::clone(&session), credentials.api_id);
    let client = Client::new(pool.handle.clone());
    let runner = tokio::spawn(pool.runner.run());

    let token = client
        .request_login_code(phone, &credentials.api_hash)
        .await
        .map_err(|e| anyhow::anyhow!("Telegram would not send a code to {phone}: {e}"))?;
    let code = prompt.code()?;
    let user = match client.sign_in(&token, code.trim()).await {
        Ok(user) => user,
        Err(SignInError::PasswordRequired(password_token)) => {
            let password = prompt.password(password_token.hint())?;
            client
                .check_password(password_token, password.trim().as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("the password was refused: {e:?}"))?
        }
        Err(SignInError::InvalidCode) => anyhow::bail!("that code was not accepted"),
        Err(SignInError::SignUpRequired) => {
            anyhow::bail!("this number has no Telegram account; Genatrix does not create one")
        }
        Err(e) => anyhow::bail!("sign-in failed: {e:?}"),
    };

    let user_id = user.id().bot_api_dialog_id_unchecked();
    let name = user.full_name();
    let snapshot = session.snapshot()?;
    drop(client);
    runner.abort();
    Ok(SignedIn {
        user_id,
        name,
        session: snapshot,
    })
}
