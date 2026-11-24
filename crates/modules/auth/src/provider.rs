//! The `ApiKey` credential check.
//!
//! Completed checks count one SHA-256 and one constant-time comparison.
//! Malformed input skips the indexed lookup, which reveals only the public
//! key format. A prefix lookup and its permission reads share a snapshot.

use crate::log_events::VERIFICATION_FAILED;
use crate::{ApiKey, Auth, config, devices, store};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use timbre_edge_crypto::{Digest, sha256};
use timbre_edge_sqlite::rusqlite::Connection;
use timbre_kernel::clock::unix_millis;
use timbre_kernel::credential::{
    Credential, CredentialProvider, RejectDetail, RejectReason, Verdict,
};
use timbre_kernel::identity::Identity;
use timbre_kernel::{BoxFuture, Ctx, Fallible, Secret, log};

/// The digest compared against when no device owns the presented prefix.
/// Its value is irrelevant; that a comparison happens at all is the point.
const DUMMY: Digest = Digest::from_bytes([0x5a; 32]);

/// How much hashing and comparing the checks have done. Timing is a
/// contract, so it is tested by counting the operations rather than by
/// measuring a wall clock that every busy machine makes lie.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Work {
    digests: AtomicU64,
    comparisons: AtomicU64,
}

#[cfg(test)]
impl Work {
    fn digest(&self) {
        self.digests.fetch_add(1, Ordering::Relaxed);
    }

    fn comparison(&self) {
        self.comparisons.fetch_add(1, Ordering::Relaxed);
    }

    /// Digests taken and comparisons made so far.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (u64, u64) {
        (
            self.digests.load(Ordering::Relaxed),
            self.comparisons.load(Ordering::Relaxed),
        )
    }
}

impl CredentialProvider for Auth {
    fn check(&self, ctx: Ctx, credential: Credential) -> BoxFuture<Verdict> {
        Box::pin(async move {
            let Credential::ApiKey(presented) = credential else {
                // The kernel routes by kind, so this is unreachable; a
                // rejection is still the only safe answer.
                return Verdict::Rejected(RejectReason::Invalid, None);
            };
            match verify(&ctx, presented).await {
                Ok(verdict) => verdict,
                Err(failure) => {
                    // Storage is unavailable: refuse rather than let anyone
                    // in, and say so with the failure's own code.
                    log!(
                        ctx,
                        warn,
                        VERIFICATION_FAILED,
                        code = failure.code().as_str()
                    );
                    Verdict::Rejected(RejectReason::Invalid, RejectDetail::try_new("unavailable"))
                }
            }
        })
    }
}

async fn verify(ctx: &Ctx, presented: Secret<String>) -> Fallible<Verdict> {
    let shared = config::shared_db(ctx)?;
    let state = crate::state(ctx)?;
    let client = ctx.client();
    let now = SystemTime::now();
    let worker = ctx.clone();
    ctx.blocking(move || {
        let database = shared.get(&worker)?;
        let outcome = database.read_transaction(|sql| {
            examine(
                &worker,
                sql,
                #[cfg(test)]
                state.work(),
                &presented,
                now,
            )
        })?;
        Ok(match outcome {
            Outcome::Accepted { prefix, identity } => {
                state.activity().note(prefix, now, client);
                Verdict::Recognized(identity)
            }
            Outcome::Malformed => rejected(RejectReason::Invalid, "malformed"),
            Outcome::Unknown => rejected(RejectReason::Invalid, "unknown"),
            Outcome::Mismatch => rejected(RejectReason::Invalid, "mismatch"),
            Outcome::Expired => rejected(RejectReason::Disabled, "expired"),
        })
    })
    .await
    .and_then(std::convert::identity)
}

/// What the stored state says about the presented key.
enum Outcome {
    Accepted { prefix: String, identity: Identity },
    Malformed,
    Unknown,
    Mismatch,
    Expired,
}

fn rejected(reason: RejectReason, label: &'static str) -> Verdict {
    Verdict::Rejected(reason, RejectDetail::try_new(label))
}

fn examine(
    ctx: &Ctx,
    sql: &Connection,
    #[cfg(test)] work: &Work,
    presented: &Secret<String>,
    now: SystemTime,
) -> Fallible<Outcome> {
    let prefix = ApiKey::parse(presented.expose()).map(|key| key.prefix());
    let candidate = match &prefix {
        Some(prefix) => devices::by_prefix(sql, prefix)?,
        None => None,
    };
    // One digest and one comparison on every path, including the ones that
    // already know the answer.
    #[cfg(test)]
    work.digest();
    let digest = sha256(presented.expose().as_bytes());
    let stored = candidate
        .as_ref()
        .and_then(|candidate| Digest::from_slice(&candidate.digest))
        .unwrap_or(DUMMY);
    #[cfg(test)]
    work.comparison();
    let matched = digest.matches(&stored);
    let Some(prefix) = prefix else {
        return Ok(Outcome::Malformed);
    };
    let Some(candidate) = candidate else {
        return Ok(Outcome::Unknown);
    };
    if !matched {
        return Ok(Outcome::Mismatch);
    }
    if candidate
        .expires
        .is_some_and(|expires| expires <= unix_millis(now))
    {
        return Ok(Outcome::Expired);
    }
    let user = crate::stored_user(&candidate.user)?;
    let permissions = crate::effective(
        ctx,
        candidate.admin,
        &store::permissions_of(sql, candidate.user_id)?,
    );
    Ok(Outcome::Accepted {
        prefix,
        identity: Identity { user, permissions },
    })
}
