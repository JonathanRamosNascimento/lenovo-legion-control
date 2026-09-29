//! polkit authorisation of D-Bus callers.
//!
//! The caller is identified by its unique bus name (`system-bus-name`
//! subject); polkit resolves its uid and session itself. Interactive
//! authentication is allowed so desktop agents can prompt for a password
//! (actions using `auth_admin_keep`).

use std::collections::HashMap;

use zbus::Connection;
use zbus::fdo;
use zbus::zvariant::Value;

/// Authorisation policy of the running daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Ask polkit for every privileged call.
    Polkit,
    /// Development mode (session bus only): allow everything.
    AllowAll,
}

/// Checks that `sender` may perform `action`.
pub async fn authorize(
    policy: Policy,
    conn: &Connection,
    sender: Option<&str>,
    action: &str,
) -> fdo::Result<()> {
    if policy == Policy::AllowAll {
        return Ok(());
    }
    let sender = sender.ok_or_else(|| fdo::Error::AccessDenied("unknown caller".into()))?;
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .await?;
    let mut subject_details: HashMap<&str, Value<'_>> = HashMap::new();
    subject_details.insert("name", Value::from(sender));
    let subject = ("system-bus-name", subject_details);
    let details: HashMap<&str, &str> = HashMap::new();
    const ALLOW_USER_INTERACTION: u32 = 1;
    let (authorized, _challenge, _): (bool, bool, HashMap<String, String>) = proxy
        .call(
            "CheckAuthorization",
            &(subject, action, details, ALLOW_USER_INTERACTION, ""),
        )
        .await
        .map_err(|e| fdo::Error::AccessDenied(format!("polkit check failed: {e}")))?;
    if authorized {
        tracing::debug!(sender, action, "authorized");
        Ok(())
    } else {
        tracing::info!(sender, action, "denied by polkit");
        Err(fdo::Error::AccessDenied(
            "Administrator permission required.".into(),
        ))
    }
}
