//! Entity types: Raw, Item, Thread, Person, Blob, Annotation.
//!
//! Design: `docs/design/01-data-model.md` (entities) and
//! `docs/design/02-trust-boundary.md` (sensitivity levels).
//!
//! This crate holds pure types and their invariants. It knows nothing
//! about storage, connectors, or models. Three layers, one direction:
//! `Raw` produces `Item`, `Item` produces `Annotation`, never the reverse.
//! No field on `Item` is ever written by a model; model output lives in
//! `Annotation`.

pub mod annotation;
pub mod blob;
pub mod id;
pub mod item;
pub mod person;
pub mod raw;
pub mod sensitivity;
pub mod source;
pub mod thread;

pub use annotation::{Annotation, AnnotationKind, Producer};
pub use blob::{Blob, ContentHash};
pub use id::{AnnotationId, BlobRef, HandleId, ItemId, PersonId, RawId, ThreadId};
pub use item::{Direction, Item, Kind, Payload, Timestamp};
pub use person::{Confidence, Handle, HandleKind, Person};
pub use raw::Raw;
pub use sensitivity::Level;
pub use source::{Connector, Source};
pub use thread::{Thread, ThreadKind};
