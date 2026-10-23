//! The Blueprint Editor engine of `app/trove/blueprint/*`: the codec's
//! file-level half (exact `BlueprintError` messages, entity sections, the v5
//! zlib container), materials, Trove Creations lint, rotate/mirror, layer
//! flattening and `.qb` import/export.

pub mod codec;
pub mod editor;
pub mod lint;
pub mod materials;
pub mod merge;
pub mod py;
pub mod qb;
pub mod transform;

#[cfg(test)]
mod tests;

/// A Python exception by class, so each caller can catch what its Python
/// counterpart catches. `Unhandled` is anything that would become a 500.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Editor(String),
    Blueprint(String),
    Transform(String),
    Merge(String),
    Qb(String),
    Unhandled(String),
}

impl Fault {
    pub fn message(&self) -> &str {
        match self {
            Fault::Editor(m)
            | Fault::Blueprint(m)
            | Fault::Transform(m)
            | Fault::Merge(m)
            | Fault::Qb(m)
            | Fault::Unhandled(m) => m,
        }
    }

    /// `except (A, B) as exc: raise EditorError(str(exc))`.
    pub fn wrap(self, catch: &[fn(&Fault) -> bool]) -> Fault {
        if catch.iter().any(|f| f(&self)) { Fault::Editor(self.message().to_owned()) } else { self }
    }

    pub fn is_blueprint(&self) -> bool {
        matches!(self, Fault::Blueprint(_))
    }

    pub fn is_transform(&self) -> bool {
        matches!(self, Fault::Transform(_))
    }

    pub fn is_merge(&self) -> bool {
        matches!(self, Fault::Merge(_))
    }

    pub fn is_qb(&self) -> bool {
        matches!(self, Fault::Qb(_))
    }
}

pub type Res<T> = Result<T, Fault>;
