//! Single authority for which supervised child owns the OCR engine cache.
//!
//! `watch` and `warmup` both compile and load TensorRT engines from
//! `cache\engines`, so at most one may run at a time. Claims and releases go
//! through one mutex: a check-then-set against two independent flags would let
//! two commands dispatched close together both observe "idle" and spawn
//! concurrently (the frontend gates the UI, but commands can still race it).

use std::sync::{Arc, Mutex};

/// Who currently holds the engine cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineUser {
    #[default]
    Idle,
    Watching,
    Warming,
}

/// The claim itself. Cloneable so reaper threads can release when their child
/// exits; managed as Tauri state so every command sees the same instance.
#[derive(Default, Clone)]
pub struct EngineUse(Arc<Mutex<EngineUser>>);

impl EngineUse {
    pub fn current(&self) -> EngineUser {
        *self.0.lock().unwrap()
    }

    /// Atomically claim the engine for `user`. On failure returns the current
    /// holder, so the caller can distinguish "join my own kind" (a second
    /// warmup request joins the in-flight run) from "someone else has it".
    pub fn try_claim(&self, user: EngineUser) -> Result<(), EngineUser> {
        let mut guard = self.0.lock().unwrap();
        if *guard == EngineUser::Idle {
            *guard = user;
            Ok(())
        } else {
            Err(*guard)
        }
    }

    /// Release a claim. Only the named holder's release resets to idle, so a
    /// late reaper cannot clear a claim someone else has since taken.
    pub fn release(&self, user: EngineUser) {
        let mut guard = self.0.lock().unwrap();
        if *guard == user {
            *guard = EngineUser::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_exclusive_and_report_the_holder() {
        let engine = EngineUse::default();
        assert_eq!(engine.current(), EngineUser::Idle);

        assert!(engine.try_claim(EngineUser::Warming).is_ok());
        assert_eq!(engine.current(), EngineUser::Warming);

        // Both a competing watch and a second warmup are refused, and each
        // learns who holds the claim.
        assert_eq!(
            engine.try_claim(EngineUser::Watching),
            Err(EngineUser::Warming)
        );
        assert_eq!(
            engine.try_claim(EngineUser::Warming),
            Err(EngineUser::Warming)
        );

        engine.release(EngineUser::Warming);
        assert!(engine.try_claim(EngineUser::Watching).is_ok());
    }

    #[test]
    fn release_is_a_no_op_for_a_non_holder() {
        let engine = EngineUse::default();
        assert!(engine.try_claim(EngineUser::Watching).is_ok());

        // A stale warmup reaper firing late must not clear the watch claim.
        engine.release(EngineUser::Warming);
        assert_eq!(engine.current(), EngineUser::Watching);

        engine.release(EngineUser::Watching);
        assert_eq!(engine.current(), EngineUser::Idle);
    }

    #[test]
    fn clones_share_one_claim() {
        let engine = EngineUse::default();
        let reaper_handle = engine.clone();
        assert!(engine.try_claim(EngineUser::Warming).is_ok());
        reaper_handle.release(EngineUser::Warming);
        assert_eq!(engine.current(), EngineUser::Idle);
    }
}
