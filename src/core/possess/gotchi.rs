use super::{Possessable, PossessionKind};
use crate::{config, AttributeParseError, Item, CONFIG};
use config::{ArchetypeHandle, ArchetypeKind};
use rusoto_dynamodb::AttributeValue;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct GotchiHarvestOwner {
    pub id: String,
    pub harvested: u64,
}
impl GotchiHarvestOwner {
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
            harvested: item
                .get("harvested")
                .ok_or(MissingField("harvested"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("harvested"))?
                .parse()
                .map_err(|e| IntFieldParse("harvested", e))?,
        })
    }
}
impl Into<AttributeValue> for GotchiHarvestOwner {
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
                        "harvested".to_string(),
                        AttributeValue {
                            n: Some(self.harvested.to_string()),
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

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Gotchi {
    archetype_handle: ArchetypeHandle,
    pub nickname: String,
    pub harvest_log: Vec<GotchiHarvestOwner>,
}
impl Possessable for Gotchi {
    fn from_possession_kind(pk: PossessionKind) -> Option<Self> {
        pk.as_gotchi()
    }
    fn into_possession_kind(self) -> PossessionKind {
        PossessionKind::Gotchi(self)
    }
}
impl std::ops::Deref for Gotchi {
    type Target = config::GotchiArchetype;

    fn deref(&self) -> &Self::Target {
        match &CONFIG
            .possession_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
            .kind
        {
            ArchetypeKind::Gotchi(g) => g,
            _ => panic!(
                "gotchi has non-gotchi archetype handle {}",
                self.archetype_handle
            ),
        }
    }
}

impl Gotchi {
    pub fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            nickname: CONFIG.possession_archetypes[archetype_handle].name.clone(),
            harvest_log: vec![GotchiHarvestOwner {
                id: owner_id.to_string(),
                harvested: 0,
            }],
        }
    }
    pub fn fill_from_item(&mut self, item: &Item) -> Result<(), AttributeParseError> {
        use AttributeParseError::*;

        self.nickname = item
            .get("nickname")
            .ok_or(MissingField("nickname"))?
            .s
            .as_ref()
            .ok_or(WronglyTypedField("nickname"))?
            .clone();
        self.harvest_log = item
            .get("harvest_log")
            .ok_or(MissingField("harvest_log"))?
            .l
            .as_ref()
            .ok_or(WronglyTypedField("harvest_log"))?
            .iter()
            .filter_map(|v| {
                match v.m.as_ref() {
                    Some(m) => match GotchiHarvestOwner::from_item(m) {
                        Ok(o) => return Some(o),
                        Err(e) => println!("error parsing item in harvest log {}", e),
                    },
                    None => println!("non-map item in harvest log"),
                };
                None
            })
            .collect();

        Ok(())
    }
    pub fn write_item(&self, item: &mut Item) {
        item.insert(
            "harvest_log".to_string(),
            AttributeValue {
                l: Some(
                    self.harvest_log
                        .iter()
                        .cloned()
                        .map(|gho| gho.into())
                        .collect(),
                ),
                ..Default::default()
            },
        );
        item.insert(
            "nickname".to_string(),
            AttributeValue {
                s: Some(self.nickname.clone()),
                ..Default::default()
            },
        );
    }
}
#[test]
fn gotchi_serialize() -> Result<(), AttributeParseError> {
    dotenv::dotenv().ok();

    let og = Gotchi::new(
        CONFIG
            .possession_archetypes
            .iter()
            .position(|x| x.name == "Adorpheus")
            .unwrap(),
        "bob",
    );

    let mut og_item = Item::new();
    og.write_item(&mut og_item);

    let mut og_copy = Gotchi::default();
    og_copy.fill_from_item(&og_item)?;

    assert_eq!(og, og_copy);

    Ok(())
}
