use std::sync::atomic::{AtomicU64, Ordering};

use once_cell::sync::Lazy;

static CONTEXT_SEQUENCE: Lazy<AtomicU64> = Lazy::new(|| AtomicU64::new(1));

#[inline(always)]
pub(crate) fn sequence() -> u64 {
    CONTEXT_SEQUENCE.fetch_add(1, Ordering::SeqCst)
}
