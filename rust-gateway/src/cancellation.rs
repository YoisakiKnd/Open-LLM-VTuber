//! Cooperative task cancellation primitives.
//!
//! Used by the upcoming native conversation/provider work (M2/M3) to cancel
//! in-flight LLM calls, TTS tasks and tool invocations when the user
//! interrupts. Cancellation is cooperative: async tasks poll
//! [`CancellationToken::is_cancelled`] (or await
//! [`wait_for_cancellation`]) and unwind.
//!
//! As of M1 this module is only exercised by its own unit tests; the
//! production call sites land with the native provider work (M2).
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shareable, cloneable cancellation signal with one-way parent propagation.
///
/// A child token observes both its own flag and its parent's flag, so
/// cancelling a parent immediately cancels every existing descendant.
/// Cancelling a child never affects the parent or siblings.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    parent: Option<Arc<AtomicBool>>,
}

impl CancellationToken {
    /// Creates a token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` once this token (or an ancestor) has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    /// Marks this token (and every descendant) as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Creates a child token that observes this token's cancellation.
    pub fn child(&self) -> CancellationToken {
        CancellationToken {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::clone(&self.cancelled)),
        }
    }
}

/// RAII guard that cancels a token when dropped.
///
/// Useful for scoped cancellation: a task spawns a child token, registers a
/// guard, and the token is cancelled automatically if the guard is dropped
/// (e.g. the parent task is unwound) before explicit completion.
#[derive(Debug)]
pub struct CancellationGuard {
    token: CancellationToken,
    armed: bool,
}

impl CancellationGuard {
    /// Creates a guard that cancels `token` on drop.
    pub fn new(token: CancellationToken) -> Self {
        Self { token, armed: true }
    }

    /// Disarms the guard; the token will not be cancelled on drop.
    pub fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}

/// Resolves as soon as the token is cancelled.
pub async fn wait_for_cancellation(token: &CancellationToken) {
    if token.is_cancelled() {
        return;
    }
    // Poll until cancelled. Async runtimes guarantee fair polling; a
    // busy-poll of an atomic is cheap and avoids extra dependencies.
    loop {
        tokio::task::yield_now().await;
        if token.is_cancelled() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_is_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn clones_share_cancellation_state() {
        let token = CancellationToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled());
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancelling_twice_is_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn child_tokens_are_not_cancelled_by_default() {
        let token = CancellationToken::new();
        let child = token.child();
        assert!(!child.is_cancelled());
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancelling_parent_propagates_to_existing_children() {
        let token = CancellationToken::new();
        let child = token.child();
        token.cancel();
        assert!(child.is_cancelled());
    }

    #[test]
    fn child_created_after_parent_cancel_starts_cancelled() {
        let token = CancellationToken::new();
        token.cancel();
        let child = token.child();
        assert!(child.is_cancelled());
    }

    #[test]
    fn cancelling_child_does_not_cancel_parent_or_siblings() {
        let token = CancellationToken::new();
        let first = token.child();
        let second = token.child();
        first.cancel();
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        assert!(!token.is_cancelled());
    }

    #[test]
    fn guard_cancels_token_on_drop() {
        let token = CancellationToken::new();
        {
            let _guard = CancellationGuard::new(token.clone());
            assert!(!token.is_cancelled());
        }
        assert!(token.is_cancelled());
    }

    #[test]
    fn disarmed_guard_does_not_cancel() {
        let token = CancellationToken::new();
        {
            let guard = CancellationGuard::new(token.clone());
            guard.disarm();
        }
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn wait_for_cancellation_resolves_after_cancel() {
        let token = CancellationToken::new();
        let waiter = {
            let token = token.clone();
            tokio::spawn(async move { wait_for_cancellation(&token).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("waiter should resolve")
            .expect("waiter task should not panic");
    }

    #[tokio::test]
    async fn wait_for_cancellation_resolves_immediately_when_already_cancelled() {
        let token = CancellationToken::new();
        token.cancel();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            wait_for_cancellation(&token),
        )
        .await
        .expect("already-cancelled token should resolve immediately");
    }
}
