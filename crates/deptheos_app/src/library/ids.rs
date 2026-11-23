//! Ids for what the history keeps: renders, stacks, batches and temporary
//! files.

/// A render's id, its directory's name under `Renders/`; parsed strictly
/// since it names a folder.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct RenderId(pub(crate) String);

impl From<RenderId> for String {
    fn from(id: RenderId) -> Self {
        id.0
    }
}

impl TryFrom<String> for RenderId {
    type Error = String;

    fn try_from(id: String) -> Result<Self, String> {
        Self::parse(&id).ok_or_else(|| format!("not a render id: {id:?}"))
    }
}

impl RenderId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An id as [`Self::as_str`] wrote it; `None` for anything this store
    /// could not have written.
    pub fn parse(id: &str) -> Option<Self> {
        is_id(id).then(|| Self(id.to_string()))
    }

    #[cfg(test)]
    pub(crate) fn for_tests(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// A stack's id, shared by its numbered renders; never shown. Parsed
/// strictly since it names a file (`Stacks/<id>.json`).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct StackId(pub(crate) String);

impl From<StackId> for String {
    fn from(id: StackId) -> Self {
        id.0
    }
}

impl TryFrom<String> for StackId {
    type Error = String;

    fn try_from(id: String) -> Result<Self, String> {
        Self::parse(&id).ok_or_else(|| format!("not a stack id: {id:?}"))
    }
}

impl StackId {
    /// A new stack: Add… to an empty list, Clear, each stack of a batch.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(new_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An id as [`Self::as_str`] wrote it; `None` for anything this store
    /// could not have written.
    pub fn parse(id: &str) -> Option<Self> {
        is_id(id).then(|| Self(id.to_string()))
    }
}

/// A fresh time-sortable id.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// Whether `name` is an id this store could have written, so nothing else
/// in the root is taken for one.
pub(crate) fn is_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}
