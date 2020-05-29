use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use std::time::SystemTime;
use core::{AttributeParseError, Category, Key, TABLE_NAME, Item, Profile};
use core::config;
use config::{ArchetypeHandle, PlantArchetype, CONFIG};
use core::possess;
use possess::{Possession, Possessed};

pub async fn exists(db: &DynamoDbClient, user_id: String) -> bool {
    db.get_item(rusoto_dynamodb::GetItemInput {
        key: Profile::key_item(user_id),
        table_name: TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map(|x| x.item.is_some())
    .unwrap_or(false)
}

pub async fn get_possession(db: &DynamoDbClient, key: Key) -> Result<Possession, String> {
    db.get_item(rusoto_dynamodb::GetItemInput {
        key: key.clone().into_item(),
        table_name: TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| format!("couldn't read {:?} from db to get possession: {}", key, e))
    .and_then(|x| {
        match Possession::from_item(
            &x.item
                .ok_or_else(|| format!("no item at {:?} to get possession for", key))?,
        ) {
            Ok(p) => Ok(p),
            Err(e) => Err(format!(
                "couldn't parse possession to get possession: {}",
                e
            )),
        }
    })
}

pub async fn get_tile(db: &DynamoDbClient, id: uuid::Uuid) -> Result<Tile, String> {
    db.get_item(rusoto_dynamodb::GetItemInput {
        key: Key::tile(id).into_item(),
        table_name: TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| format!("couldn't read {:?} from db to get tile: {}", id, e))
    .and_then(|x| {
        match Tile::from_item(
            &x.item
                .ok_or_else(|| format!("no item at {:?} to get possession for", id))?,
        ) {
            Ok(p) => Ok(p),
            Err(e) => Err(format!(
                "couldn't parse possession to get possession: {}",
                e
            )),
        }
    })
}

/// This function empties someone's profile, setting their xp to zero.
pub async fn goblin_slaughter(db: &DynamoDbClient) -> Result<(), String> {
    let profiles = Profile::fetch_all(db).await?;

    db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
        request_items: [(
            TABLE_NAME.to_string(),
            profiles
                .into_iter()
                .map(|mut p| rusoto_dynamodb::WriteRequest {
                    put_request: Some(rusoto_dynamodb::PutRequest {
                        item: {
                            p.last_farm = std::time::SystemTime::now();
                            p.xp = 0;
                            p.item()
                        },
                    }),
                    ..Default::default()
                })
                .collect(),
        )]
        .iter()
        .cloned()
        .collect(),
        ..Default::default()
    })
    .await
    .map_err(|e| format!("couldn't write wiped profiles into db: {}", e))?;

    Ok(())
}

/// This function removes the plants from all tiles.
pub async fn goblin_stomp(
    db: &DynamoDbClient,
    to_farming: &crossbeam_channel::Sender<super::FarmingInputEvent>,
) -> Result<(), String> {
    let tiles = Tile::fetch_all(db).await?;

    db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
        request_items: [(
            TABLE_NAME.to_string(),
            tiles
                .into_iter()
                .map(|mut tile| rusoto_dynamodb::WriteRequest {
                    put_request: Some(rusoto_dynamodb::PutRequest {
                        item: {
                            to_farming
                                .send(super::FarmingInputEvent::ActivateUser(tile.steader.clone()))
                                .unwrap();
                            tile.plant.take();

                            tile.into_av().m.expect("tile attribute should be map")
                        },
                    }),
                    ..Default::default()
                })
                .collect(),
        )]
        .iter()
        .cloned()
        .collect(),
        ..Default::default()
    })
    .await
    .map_err(|e| format!("couldn't write new land into db: {}", e))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct Tile {
    pub acquired: SystemTime,
    pub plant: Option<Plant>,
    pub id: uuid::Uuid,
    pub steader: String,
}
impl Tile {
    pub fn new(steader: String) -> Tile {
        Tile {
            acquired: SystemTime::now(),
            plant: None,
            id: uuid::Uuid::new_v4(),
            steader,
        }
    }

    pub async fn fetch_all(db: &DynamoDbClient) -> Result<Vec<Tile>, String> {
        let query = db
            .query(rusoto_dynamodb::QueryInput {
                table_name: TABLE_NAME.to_string(),
                key_condition_expression: Some("cat = :land_cat".to_string()),
                expression_attribute_values: Some(
                    [(":land_cat".to_string(), Category::Land.into_av())]
                        .iter()
                        .cloned()
                        .collect(),
                ),
                ..Default::default()
            })
            .await;

        Ok(query
            .map_err(|e| format!("Couldn't search land category: {}", e))?
            .items
            .ok_or_else(|| format!("land query returned no items"))?
            .iter_mut()
            .filter_map(|i| match Tile::from_item(i) {
                Ok(tile) => Some(tile),
                Err(e) => {
                    println!("error parsing tile: {}", e);
                    None
                }
            })
            .collect())
    }

    pub fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        Ok(Self {
            acquired: humantime::parse_rfc3339(
                item.get("acquired")
                    .ok_or(MissingField("acquired"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("acquired"))?,
            )
            .map_err(|e| TimeFieldParse("acquired", e))?,
            plant: match item.get("plant") {
                Some(av) => Some(Plant::from_av(av)?),
                None => None,
            },
            id: uuid::Uuid::parse_str(
                item.get("id")
                    .ok_or(MissingField("id"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("id"))?,
            )
            .map_err(|e| IdFieldParse("id", e))?,
            steader: item
                .get("steader")
                .ok_or(MissingField("steader"))?
                .s
                .as_ref()
                .ok_or(WronglyTypedField("steader"))?
                .clone(),
        })
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            m: Some({
                let mut m = Key::tile(self.id.clone()).into_item();
                m.insert(
                    "acquired".to_string(),
                    AttributeValue {
                        s: Some(humantime::format_rfc3339(self.acquired).to_string()),
                        ..Default::default()
                    },
                );
                m.insert(
                    "steader".to_string(),
                    AttributeValue {
                        s: Some(self.steader.clone()),
                        ..Default::default()
                    },
                );

                if let Some(plant) = self.plant {
                    m.insert("plant".to_string(), plant.into_av());
                }

                m
            }),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Craft {
    pub until_finish: f32,
    pub total_cycles: f32,
    pub destroys_plant: bool,
    pub makes: ArchetypeHandle,
}

#[derive(Debug, Clone)]
pub struct Plant {
    pub xp: u64,
    pub until_yield: f32,
    pub craft: Option<Craft>,
    pub pedigree: Vec<possess::seed::SeedGrower>,
    pub archetype_handle: ArchetypeHandle,
}

impl std::ops::Deref for Plant {
    type Target = PlantArchetype;

    fn deref(&self) -> &Self::Target {
        &CONFIG
            .plant_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
    }
}
impl Craft {
    pub fn from_av(av: &AttributeValue) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        let m = av.m.as_ref().ok_or(WrongType)?;

        Ok(Self {
            until_finish: m
                .get("until_finish")
                .ok_or(MissingField("until_finish"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("until_finish"))?
                .parse()
                .map_err(|e| FloatFieldParse("until_finish", e))?,
            total_cycles: m
                .get("total_cycles")
                .ok_or(MissingField("total_cycles"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("total_cycles"))?
                .parse()
                .map_err(|e| FloatFieldParse("total_cycles", e))?,
            makes: m
                .get("makes")
                .ok_or(MissingField("makes"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("makes"))?
                .parse()
                .map_err(|e| IntFieldParse("makes", e))?,
            destroys_plant: match m.get("destroys_plant") {
                None => false,
                Some(x) => x.bool.ok_or(WronglyTypedField("destroys_plant"))?,
            },
        })
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            m: Some(
                [
                    (
                        "until_finish".to_string(),
                        AttributeValue {
                            n: Some(self.until_finish.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "total_cycles".to_string(),
                        AttributeValue {
                            n: Some(self.total_cycles.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "makes".to_string(),
                        AttributeValue {
                            n: Some(self.makes.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "destroys_plant".to_string(),
                        AttributeValue {
                            bool: Some(self.destroys_plant),
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

impl Plant {
    pub fn from_seed(seed: Possessed<possess::Seed>) -> Self {
        let mut s = Self {
            xp: 0,
            until_yield: 0.0,
            craft: None,
            archetype_handle: CONFIG.find_plant_handle(&seed.inner.grows_into).unwrap(),
            pedigree: seed.inner.pedigree,
        };
        s.until_yield = s.base_yield_duration;
        s
    }

    pub fn current_advancement(&self) -> &config::PlantAdvancement {
        self.advancements.current(self.xp)
    }

    pub fn next_advancement(&self) -> Option<&config::PlantAdvancement> {
        self.advancements.next(self.xp)
    }

    pub fn increment_xp(&mut self) -> Option<&'static config::PlantAdvancement> {
        CONFIG
            .plant_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
            .advancements
            .increment_xp(&mut self.xp)
    }

    pub fn from_av(av: &AttributeValue) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        let m = av.m.as_ref().ok_or(WrongType)?;

        Ok(Self {
            xp: m
                .get("xp")
                .ok_or(MissingField("xp"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("xp"))?
                .parse()
                .map_err(|e| IntFieldParse("xp", e))?,
            until_yield: m
                .get("until_yield")
                .ok_or(MissingField("until_yield"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("until_yield"))?
                .parse()
                .map_err(|e| FloatFieldParse("until_yield", e))?,
            archetype_handle: m
                .get("archetype_handle")
                .ok_or(MissingField("archetype_handle"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("archetype_handle"))?
                .parse()
                .map_err(|e| IntFieldParse("archetype_handle", e))?,
            craft: match m.get("craft") {
                Some(c) => Some(Craft::from_av(c)?),
                None => None,
            },
            pedigree: m
                .get("pedigree")
                .ok_or(MissingField("pedigree"))?
                .l
                .as_ref()
                .ok_or(WronglyTypedField("pedigree"))?
                .iter()
                .filter_map(|v| {
                    match v.m.as_ref() {
                        Some(m) => match possess::seed::SeedGrower::from_item(m) {
                            Ok(s) => return Some(s),
                            Err(e) => println!("error parsing pedigree item: {}", e),
                        },
                        None => println!("non-map item in pedigree"),
                    };
                    None
                })
                .collect(),
        })
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            m: Some({
                let mut attrs: Item = [
                    (
                        "xp".to_string(),
                        AttributeValue {
                            n: Some(self.xp.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "until_yield".to_string(),
                        AttributeValue {
                            n: Some(self.until_yield.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "archetype_handle".to_string(),
                        AttributeValue {
                            n: Some(self.archetype_handle.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "pedigree".to_string(),
                        AttributeValue {
                            l: Some(self.pedigree.iter().cloned().map(|sg| sg.into()).collect()),
                            ..Default::default()
                        },
                    ),
                ]
                .iter()
                .cloned()
                .collect();

                if let Some(craft) = self.craft {
                    attrs.insert("craft".to_string(), craft.into_av());
                }

                attrs
            }),
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub struct Hacksteader {
    pub user_id: String,
    pub profile: Profile,
    pub land: Vec<Tile>,
    pub inventory: Vec<Possession>,
    pub gotchis: Vec<Possessed<possess::Gotchi>>,
}
impl Hacksteader {
    pub async fn new_in_db(db: &DynamoDbClient, user_id: String) -> Result<(), String> {
        // just give them a profile for now
        db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
            request_items: [(
                TABLE_NAME.to_string(),
                vec![
                    Profile::new(user_id.clone()).item(),
                    Tile::new(user_id.clone()).into_av().m.unwrap(),
                ]
                .into_iter()
                .map(|item| rusoto_dynamodb::WriteRequest {
                    put_request: Some(rusoto_dynamodb::PutRequest { item }),
                    ..Default::default()
                })
                .collect(),
            )]
            .iter()
            .cloned()
            .collect(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("couldn't add profile: {}", e))?;

        Ok(())
    }

    pub fn neighbor_bonuses(
        &self,
    ) -> Vec<(
        uuid::Uuid,
        config::ArchetypeHandle,
        (config::PlantAdvancement, config::PlantAdvancementKind),
    )> {
        use config::PlantAdvancementKind::*;

        self.land
            .iter()
            .filter_map(|tile| Some((tile.id, tile.plant.as_ref()?)))
            .flat_map(|(steader, plant)| {
                plant
                    .advancements
                    .unlocked(plant.xp)
                    .filter_map(move |adv| {
                        Some((
                            steader.clone(),
                            plant.archetype_handle,
                            match &adv.kind {
                                Neighbor(a) => Some((adv.clone(), *a.clone())),
                                _ => None,
                            }?,
                        ))
                    })
            })
            .collect()
    }

    pub async fn give_possession(
        db: &DynamoDbClient,
        user_id: String,
        possession: &Possession,
    ) -> Result<(), RusotoError<PutItemError>> {
        let mut new_poss = possession.clone();
        new_poss.steader = user_id;
        db.put_item(rusoto_dynamodb::PutItemInput {
            item: possession.item(),
            table_name: TABLE_NAME.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
    }

    #[allow(dead_code)]
    pub async fn delete(db: &DynamoDbClient, key: Key) -> Result<(), String> {
        match db
            .delete_item(rusoto_dynamodb::DeleteItemInput {
                key: key.into_item(),
                table_name: TABLE_NAME.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("couldn't delete in db: {}", e)),
        }
    }

    #[allow(dead_code)]
    pub async fn take(db: &DynamoDbClient, key: Key) -> Result<Possession, String> {
        match db
            .delete_item(rusoto_dynamodb::DeleteItemInput {
                key: key.into_item(),
                table_name: TABLE_NAME.to_string(),
                return_values: Some("ALL_OLD".to_string()),
                ..Default::default()
            })
            .await
        {
            Ok(rusoto_dynamodb::DeleteItemOutput {
                attributes: Some(item),
                ..
            }) => Possession::from_item(&item)
                .map_err(|e| format!("couldn't parse value returned from delete: {}", e)),
            Err(e) => Err(format!("couldn't delete in db: {}", e)),
            _ => Err(format!("no attributes returned!")),
        }
    }

    pub async fn transfer_possession(
        db: &DynamoDbClient,
        new_owner: String,
        acquisition: possess::Acquisition,
        key: Key,
    ) -> Result<(), String> {
        db.update_item(rusoto_dynamodb::UpdateItemInput {
            table_name: TABLE_NAME.to_string(),
            key: key.into_item(),
            update_expression: Some(
                concat!(
                    "SET ",
                    "steader = :new_owner, ",
                    "ownership_log = list_append(ownership_log, :ownership_entry)",
                )
                .to_string(),
            ),
            expression_attribute_values: Some(
                [
                    (
                        ":new_owner".to_string(),
                        AttributeValue {
                            s: Some(new_owner.clone()),
                            ..Default::default()
                        },
                    ),
                    (
                        ":ownership_entry".to_string(),
                        AttributeValue {
                            l: Some(vec![possess::Owner {
                                id: new_owner.clone(),
                                acquisition,
                            }
                            .into()]),
                            ..Default::default()
                        },
                    ),
                ]
                .iter()
                .cloned()
                .collect(),
            ),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("Couldn't transfer ownership in database: {}", e))?;

        Ok(())
    }

    pub async fn from_db(db: &DynamoDbClient, user_id: String) -> Result<Self, String> {
        let query = db
            .query(rusoto_dynamodb::QueryInput {
                table_name: TABLE_NAME.to_string(),
                key_condition_expression: Some("steader = :steader_id".to_string()),
                index_name: Some("steader_index".to_string()),
                expression_attribute_values: Some({
                    [(
                        ":steader_id".to_string(),
                        AttributeValue {
                            s: Some(user_id.clone()),
                            ..Default::default()
                        },
                    )]
                    .iter()
                    .cloned()
                    .collect()
                }),
                ..Default::default()
            })
            .await;
        let items = query
            .map_err(|e| format!("couldn't profile query: {}", e))?
            .items
            .ok_or_else(|| format!("no items returned from profile query"))?;

        let mut profile = None;
        let mut gotchis = Vec::new();
        let mut inventory = Vec::new();
        let mut land = Vec::new();

        for item in items.iter() {
            use AttributeParseError::*;

            match Category::from_av(
                item.get("cat")
                    .ok_or_else(|| format!("{}", MissingField("cat")))?,
            )
            .map_err(|e| format!("error parsing hacksteader item category: {}", e))?
            {
                Category::Profile => {
                    profile = Some(
                        Profile::from_item(item)
                            .map_err(|e| format!("profile parse err: {}", e))?,
                    )
                }
                Category::Gotchi => gotchis.push(
                    Possessed::from_possession(
                        Possession::from_item(item)
                            .map_err(|e| format!("gotchi parse err: {}", e))?,
                    )
                    .ok_or_else(|| format!("possession in gotchi category but not gotchi"))?,
                ),
                Category::Misc => inventory.push(
                    Possession::from_item(item)
                        .map_err(|e| format!("misc inv. item parse err: {}", e))?,
                ),
                Category::Land => {
                    land.push(Tile::from_item(item).map_err(|e| format!("tile parse err: {}", e))?)
                }
                _ => unreachable!(),
            }
        }

        Ok(Hacksteader {
            profile: profile.ok_or_else(|| format!("No profile found for {}", user_id))?,
            user_id,
            gotchis,
            inventory,
            land,
        })
    }
}
