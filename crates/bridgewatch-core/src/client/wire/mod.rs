//! The shapes the providers' APIs really return.
//!
//! One module per provider, each one decode-only: a wire type exists to be
//! deserialised from a response body and then converted into the
//! provider-neutral [`crate::model`], which is what everything above the client
//! reads. Nothing here is serialised, and nothing above the client sees a wire
//! type.
//!
//! ⛔ The split is not cosmetic. [`crate::model`] used to BE the GitLab wire
//! format, so a second provider could only ever have forged GitLab-shaped
//! values. Keeping the two apart means a provider's oddities (GitHub splits one
//! status into `status` plus `conclusion`, wraps collections in an object, and
//! has no bridges at all) are absorbed in its own `From` impl rather than
//! leaking into the verdict engine.

pub mod gitlab;
