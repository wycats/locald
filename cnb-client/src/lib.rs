//! Cloud Native Buildpacks (CNB) Client Library.
//!
//! This library provides a high-level interface for interacting with the CNB lifecycle.
//! It builds upon `locald-oci` for low-level OCI operations.

#![allow(missing_docs)]
#![allow(clippy::missing_errors_doc)]

pub mod lifecycle;
pub mod runtime;
