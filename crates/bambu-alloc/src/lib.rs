//! Process heap: the sibling mimalloc rewrite (`mimalloc-core` / v3.5.1).
//!
//! Binaries and integration tests depend on this crate so there is exactly one
//! `#[global_allocator]`. First-party code stays `forbid(unsafe_code)`; the
//! allocator crate owns the `unsafe` `GlobalAlloc` impl.

use mimalloc_core::Mimalloc;

#[global_allocator]
static ALLOC: Mimalloc = Mimalloc;
