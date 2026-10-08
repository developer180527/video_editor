//! Stable identities. A ULID per object: unique across machines (so merged
//! or collaborative edits never collide), sortable by creation time, and the
//! same in every save file.

use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

macro_rules! id_type {
    ($($(#[$m:meta])* $name:ident),* $(,)?) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Ulid);

        impl $name {
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self { $name(Ulid::generate()) }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
    )*};
}

id_type!(AssetId, SequenceId, TrackId, ClipId, EffectId, LinkId);
