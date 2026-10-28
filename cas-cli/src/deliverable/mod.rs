//! Branded deliverables core behind `cas deliverable`.
//!
//! Each verb is a module with a typed request and a receipt that carries its
//! own findings. The CLI prints the receipt and applies one exit contract:
//! 0 pass, 1 finding, 2 usage or environment ([`UsageError`]). The sibling
//! verbs share this contract and keep their typed requests private to their
//! execution paths. Scripts a verb lifts from a skill
//! ship once, embedded in the binary, through [`asset`].

pub mod asset;
pub mod brand;
pub mod capture;
pub mod finalize;
pub mod pairs;

use serde::{Deserialize, Serialize};

/// Bad arguments or a missing runtime (python3, an unreadable root): exit 2.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

/// Who reads the deliverable. Customer output never wears the bundled
/// Petrastella defaults and gets the stricter hygiene of later verbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    Internal,
    Customer,
}

impl Audience {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Customer => "customer",
        }
    }
}

/// A reason the deliverable must not ship as is: exit 1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub code: String,
    pub message: String,
}

/// What every verb's receipt exposes to the shared exit contract.
pub trait Receipt: Serialize {
    fn findings(&self) -> &[Finding];
    fn warnings(&self) -> &[String];
}
