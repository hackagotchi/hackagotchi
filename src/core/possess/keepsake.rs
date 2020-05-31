use super::{Possessable, PossessionKind};
use crate::{config, AttributeParseError, Item, CONFIG};
use config::{ArchetypeHandle, ArchetypeKind};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Keepsake {
    archetype_handle: ArchetypeHandle,
}
impl std::ops::Deref for Keepsake {
    type Target = config::KeepsakeArchetype;

    fn deref(&self) -> &Self::Target {
        match &CONFIG
            .possession_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
            .kind
        {
            ArchetypeKind::Keepsake(k) => k,
            _ => panic!(
                "keepsake has non-keepsake archetype handle {}",
                self.archetype_handle
            ),
        }
    }
}
impl Possessable for Keepsake {
    fn from_possession_kind(pk: PossessionKind) -> Option<Self> {
        pk.as_keepsake()
    }
    fn into_possession_kind(self) -> PossessionKind {
        PossessionKind::Keepsake(self)
    }
}
impl Keepsake {
    pub fn new(archetype_handle: ArchetypeHandle, _owner_id: &str) -> Self {
        Self { archetype_handle }
    }
    pub fn fill_from_item(&mut self, _item: &Item) -> Result<(), AttributeParseError> {
        Ok(())
    }
    pub fn write_item(&self, _item: &mut Item) {}
}
