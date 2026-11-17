//! `aurora-api push rotate-vapid [--dry-run]` and `aurora-api push status [--json]` (C-833).
//! Output never contains key material.
use super::vapid::{self, Rotation};
use crate::app::AppState;

/// Exit status of `rotate-vapid` with an operator-managed key: nothing to rotate here.
pub const EXIT_OPERATOR_KEY: u8 = 3;

/// Run `rotate-vapid`; the return value is the process exit status.
pub async fn rotate(state: &AppState, dry_run: bool) -> Result<u8, String> {
    match vapid::rotate(state, dry_run)
        .await
        .map_err(|e| e.message().to_owned())?
    {
        Rotation::Disabled => {
            println!("push.enabled is false: there is no key to rotate.");
            Ok(1)
        }
        Rotation::OperatorManaged => {
            eprintln!(
                "The VAPID key is operator-managed (push.vapid.private_key_env): replace the \
                 environment variable and restart the API instead."
            );
            Ok(EXIT_OPERATOR_KEY)
        }
        Rotation::DryRun { old } => {
            match old {
                Some(id) => println!("Would retire key {id} and generate a new active key."),
                None => println!("No active key yet; the first API start generates one."),
            }
            Ok(0)
        }
        Rotation::Rotated { old, new } => {
            println!("Generated active key {new}.");
            match old {
                Some(id) => println!(
                    "Key {id} is retired; its subscriptions keep working until their browsers \
                     re-subscribe or it is destroyed after 30 days."
                ),
                None => println!("There was no previous key."),
            }
            Ok(0)
        }
    }
}

pub async fn status(state: &AppState, json: bool) -> Result<(), String> {
    let st = vapid::status(state)
        .await
        .map_err(|e| e.message().to_owned())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&st).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    println!("mode: {}", st.mode);
    for k in &st.keys {
        println!(
            "key {} {} created {} subscriptions {}",
            k.id,
            k.status,
            k.created_at.to_rfc3339(),
            k.subscriptions
        );
    }
    for (s, n) in &st.subscriptions_by_state {
        println!("state {s}: {n}");
    }
    for (h, n) in &st.subscriptions_by_host {
        println!("host {h}: {n}");
    }
    Ok(())
}
