use super::{Possessable, PossessionKind};
use crate::{config, AttributeParseError, Item, CONFIG};
use config::{ArchetypeHandle, ArchetypeKind};
use rusoto_dynamodb::AttributeValue;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Seed {
    pub archetype_handle: ArchetypeHandle,
    pub pedigree: Vec<SeedGrower>,
}
impl Possessable for Seed {
    fn from_possession_kind(pk: PossessionKind) -> Option<Self> {
        pk.as_seed()
    }
    fn into_possession_kind(self) -> PossessionKind {
        PossessionKind::Seed(self)
    }
}
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct SeedGrower {
    pub id: String,
    pub generations: u64,
}
impl SeedGrower {
    pub fn new(id: String, generations: u64) -> Self {
        SeedGrower { id, generations }
    }

    pub fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        Ok(Self {
            id: item
                .get("id")
                .ok_or(MissingField("id"))?
                .s
                .as_ref()
                .ok_or(WronglyTypedField("id"))?
                .clone(),
            generations: item
                .get("generations")
                .ok_or(MissingField("generations"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("generations"))?
                .parse()
                .map_err(|e| IntFieldParse("generations", e))?,
        })
    }
}
impl Into<AttributeValue> for SeedGrower {
    fn into(self) -> AttributeValue {
        AttributeValue {
            m: Some(
                [
                    (
                        "id".to_string(),
                        AttributeValue {
                            s: Some(self.id.clone()),
                            ..Default::default()
                        },
                    ),
                    (
                        "generations".to_string(),
                        AttributeValue {
                            n: Some(self.generations.to_string()),
                            ..Default::default()
                        },
                    ),
                ]
                .iter()
                .cloned()
                .collect(),
            ),
            ..Default::default()
        }
    }
}

impl std::ops::Deref for Seed {
    type Target = config::SeedArchetype;

    fn deref(&self) -> &Self::Target {
        match CONFIG
            .possession_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
            .kind
        {
            ArchetypeKind::Seed(ref s) => s,
            _ => panic!("archetype kind corresponds to archetype of a different type"),
        }
    }
}
impl Seed {
    pub fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            pedigree: vec![SeedGrower {
                id: owner_id.to_string(),
                generations: 0,
            }],
        }
    }
    pub fn fill_from_item(&mut self, item: &Item) -> Result<(), AttributeParseError> {
        use AttributeParseError::*;

        self.pedigree = item
            .get("pedigree")
            .ok_or(MissingField("pedigree"))?
            .l
            .as_ref()
            .ok_or(WronglyTypedField("pedigree"))?
            .iter()
            .filter_map(|v| {
                match v.m.as_ref() {
                    Some(m) => match SeedGrower::from_item(m) {
                        Ok(s) => return Some(s),
                        Err(e) => println!("error parsing pedigree item: {}", e),
                    },
                    None => println!("non-map item in pedigree"),
                };
                None
            })
            .collect();

        Ok(())
    }
    pub fn write_item(&self, item: &mut Item) {
        item.insert(
            "pedigree".to_string(),
            AttributeValue {
                l: Some(self.pedigree.iter().cloned().map(|sg| sg.into()).collect()),
                ..Default::default()
            },
        );
    }
}
