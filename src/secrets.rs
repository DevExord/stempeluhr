// SPDX-License-Identifier: GPL-3.0-or-later
//! API token storage in the system keyring (Secret portal inside Flatpak,
//! Secret Service outside). The token never touches GSettings or logs.

use std::collections::HashMap;

use crate::runtime;

const SCHEMA: &str = "de.example.Stempeluhr.Token";

fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("xdg:schema", SCHEMA)])
}

#[derive(Debug)]
pub struct SecretError(pub String);

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Runs a keyring operation on the runtime. A fresh keyring session is
/// negotiated per attempt; the Secret Service key exchange occasionally fails
/// with a padding error, which goes away on the next attempt.
async fn guarded<T, F, Fut>(op: F) -> Result<T, SecretError>
where
    T: Send + 'static,
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, oo7::Error>> + Send + 'static,
{
    let mut last = SecretError(String::new());
    for attempt in 1..=3 {
        match runtime::spawn(op()).await {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => last = SecretError(e.to_string()),
            Err(e) => last = SecretError(e.detail),
        }
        tracing::debug!(attempt, error = %last, "keyring attempt failed");
    }
    Err(last)
}

pub async fn load() -> Result<Option<String>, SecretError> {
    guarded(|| async {
        let keyring = oo7::Keyring::new().await?;
        let items = keyring.search_items(&attributes()).await?;
        match items.first() {
            Some(item) => {
                let secret = item.secret().await?;
                Ok(secret.as_str().map(str::to_owned))
            }
            None => Ok(None),
        }
    })
    .await
}

pub async fn store(token: String) -> Result<(), SecretError> {
    guarded(move || {
        let token = token.clone();
        async move {
            let keyring = oo7::Keyring::new().await?;
            keyring
                .create_item(
                    "Stempeluhr – Kimai API token",
                    &attributes(),
                    token.as_str(),
                    true,
                )
                .await
        }
    })
    .await
}

pub async fn clear() -> Result<(), SecretError> {
    guarded(|| async {
        let keyring = oo7::Keyring::new().await?;
        keyring.delete(&attributes()).await
    })
    .await
}
