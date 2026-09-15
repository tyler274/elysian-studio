//! Process heap: the sibling mimalloc rewrite (`elymalloc-core`).
//!
//! Binaries and integration tests depend on this crate so there is exactly one
//! `#[global_allocator]`. First-party code stays `forbid(unsafe_code)`; the
//! allocator crate owns the `unsafe` `GlobalAlloc` impl.

use mimalloc_core::ElyMalloc;

#[global_allocator]
static ALLOC: ElyMalloc = ElyMalloc;
