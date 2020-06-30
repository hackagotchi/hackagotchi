use config::{ArchetypeHandle, PlantArchetype, CONFIG};
use hcor::config;
use hcor::possess;
use hcor::{AttributeParseError, Category, Item, Key, Profile, TABLE_NAME};
use log::*;
use possess::{Possessed, Possession};
use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use std::time::SystemTime;

pub async fn exists(db: &DynamoDbClient, user_id: String) -> bool {
    db.get_item(rusoto_dynamodb::GetItemInput {
        key: Profile::key_item(user_id),
        table_name: TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map(|x| x.item.is_some())
    .unwrap_or_else(|e| {
        error!("couldn't see if hacksteader exists: {}", e);
        false
    })
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
    pub recipe_archetype_handle: ArchetypeHandle,
}

#[derive(Debug, Clone, Default)]
pub struct Plant {
    pub xp: u64,
    pub until_yield: f32,
    pub craft: Option<Craft>,
    pub pedigree: Vec<possess::seed::SeedGrower>,
    /// Effects from potions, warp powder, etc. that actively change the behavior of this plant.
    pub effects: Vec<Effect>,
    pub archetype_handle: ArchetypeHandle,
    /// This field isn't saved to the database, and is just used
    /// when `plant.increase_xp()` is called.
    pub queued_xp_bonus: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Effect {
    pub until_finish: Option<f32>,
    /// The archetype of the item that was consumed to apply this effect.
    pub item_archetype_handle: ArchetypeHandle,
    /// The archetype of the effect within this item that describes this effect.
    pub effect_archetype_handle: ArchetypeHandle,
}

impl std::ops::Deref for Effect {
    type Target = config::Archetype;

    fn deref(&self) -> &Self::Target {
        &CONFIG
            .possession_archetypes
            .get(self.item_archetype_handle)
            .expect("invalid archetype handle")
    }
}

impl Effect {
    pub fn from_av(av: &AttributeValue) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        let m = av.m.as_ref().ok_or(WrongType)?;

        Ok(Self {
            until_finish: match m.get("until_finish") {
                Some(x) => Some(
                    x.n.as_ref()
                        .ok_or(WronglyTypedField("until_finish"))?
                        .parse()
                        .map_err(|e| FloatFieldParse("until_finish", e))?,
                ),
                None => None,
            },
            item_archetype_handle: m
                .get("item_archetype_handle")
                .ok_or(MissingField("item_archetype_handle"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("item_archetype_handle"))?
                .parse()
                .map_err(|e| IntFieldParse("item_archetype_handle", e))?,
            effect_archetype_handle: m
                .get("effect_archetype_handle")
                .ok_or(MissingField("effect_archetype_handle"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("effect_archetype_handle"))?
                .parse()
                .map_err(|e| IntFieldParse("effect_archetype_handle", e))?,
        })
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            m: Some({
                let mut a = vec![
                    (
                        "item_archetype_handle".to_string(),
                        AttributeValue {
                            n: Some(self.item_archetype_handle.to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "effect_archetype_handle".to_string(),
                        AttributeValue {
                            n: Some(self.effect_archetype_handle.to_string()),
                            ..Default::default()
                        },
                    ),
                ];

                if let Some(until_finish) = self.until_finish {
                    a.push((
                        "until_finish".to_string(),
                        AttributeValue {
                            n: Some(until_finish.to_string()),
                            ..Default::default()
                        },
                    ));
                }

                a.iter().cloned().collect()
            }),
            ..Default::default()
        }
    }
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
            recipe_archetype_handle: m
                .get("recipe_archetype_handle")
                .ok_or(MissingField("until_finish"))
                .and_then(|o| {
                    Ok(o.n
                        .as_ref()
                        .ok_or(WronglyTypedField("recipe_archetype_handle"))?
                        .parse()
                        .map_err(|e| IntFieldParse("recipe_archetype_handle", e))?)
                })
                .unwrap_or(0),
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
                        "recipe_archetype_handle".to_string(),
                        AttributeValue {
                            n: Some(self.recipe_archetype_handle.to_string()),
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
            archetype_handle: CONFIG.find_plant_handle(&seed.inner.grows_into).unwrap(),
            pedigree: seed.inner.pedigree,
            ..Default::default()
        };
        s.until_yield = s.base_yield_duration.unwrap_or(0.0);
        s
    }

    fn effect_advancements<'a>(&'a self) -> impl Iterator<Item = &'a config::PlantAdvancement> {
        self.effects
            .iter()
            .filter_map(|e| {
                CONFIG.get_item_application_plant_advancement(
                    e.item_archetype_handle,
                    e.effect_archetype_handle,
                )
            })
            .map(|(_effect, adv)| adv)
    }
    /// Excludes neighbor bonuses
    pub fn advancements_sum<'a>(
        &'a self,
        extra_advancements: impl Iterator<Item = &'a config::PlantAdvancement>,
    ) -> config::PlantAdvancementSum {
        self.advancements.sum(
            self.xp,
            self.effect_advancements().chain(extra_advancements),
        )
    }
    /// A sum struct for all of the possible advancements for this plant,
    /// plus any effects it has active.
    pub fn advancements_max_sum<'a>(
        &'a self,
        extra_advancements: impl Iterator<Item = &'a config::PlantAdvancement>,
    ) -> config::PlantAdvancementSum {
        self.advancements
            .max(self.effect_advancements().chain(extra_advancements))
    }

    pub fn neighborless_advancements_sum<'a>(
        &'a self,
        extra_advancements: impl Iterator<Item = &'a config::PlantAdvancement>,
    ) -> config::PlantAdvancementSum {
        self.advancements.raw_sum(
            self.xp,
            self.effect_advancements().chain(extra_advancements),
        )
    }

    pub fn unlocked_advancements<'a>(
        &'a self,
        extra_advancements: impl Iterator<Item = &'a config::PlantAdvancement>,
    ) -> impl Iterator<Item = &'a config::PlantAdvancement> {
        self.advancements
            .unlocked(self.xp)
            .chain(self.effect_advancements())
            .chain(extra_advancements)
    }
    pub fn all_advancements<'a>(
        &'a self,
        extra_advancements: impl Iterator<Item = &'a config::PlantAdvancement>,
    ) -> impl Iterator<Item = &'a config::PlantAdvancement> {
        self.advancements
            .all()
            .chain(self.effect_advancements())
            .chain(extra_advancements)
    }

    pub fn current_advancement(&self) -> &config::PlantAdvancement {
        self.advancements.current(self.xp)
    }

    pub fn next_advancement(&self) -> Option<&config::PlantAdvancement> {
        self.advancements.next(self.xp)
    }

    pub fn increase_xp(&mut self, mut amt: u64) -> Option<&'static config::PlantAdvancement> {
        amt += self.queued_xp_bonus;
        self.queued_xp_bonus = 0;
        CONFIG
            .plant_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
            .advancements
            .increase_xp(&mut self.xp, amt)
    }

    pub fn current_recipe_raw(&self) -> Option<config::Recipe<ArchetypeHandle>> {
        self.craft
            .as_ref()
            .and_then(|c| self.get_recipe_raw(c.recipe_archetype_handle))
    }

    pub fn current_recipe(&self) -> Option<config::Recipe<&'static config::Archetype>> {
        self.current_recipe_raw().and_then(|x| x.lookup_handles())
    }

    pub fn get_recipe_raw(
        &self,
        recipe_ah: ArchetypeHandle,
    ) -> Option<config::Recipe<ArchetypeHandle>> {
        self.advancements_sum(std::iter::empty())
            .recipes
            .get(recipe_ah)
            .cloned()
    }

    pub fn get_recipe(
        &self,
        recipe_ah: ArchetypeHandle,
    ) -> Option<config::Recipe<&'static config::Archetype>> {
        self.get_recipe_raw(recipe_ah)
            .and_then(|x| x.lookup_handles())
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
            effects: match m.get("effects") {
                Some(e) => {
                    e.l.as_ref()
                        .ok_or(WronglyTypedField("effects"))?
                        .iter()
                        .filter_map(|v| match Effect::from_av(v) {
                            Ok(s) => return Some(s),
                            Err(e) => {
                                log::error!("error parsing effects item: {}", e);
                                None
                            }
                        })
                        .collect::<Vec<Effect>>()
                }
                None => Default::default(),
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
            queued_xp_bonus: 0,
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
                        "effects".to_string(),
                        AttributeValue {
                            l: Some(self.effects.iter().cloned().map(|e| e.into_av()).collect()),
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

#[derive(Clone, Debug)]
pub struct NeighborBonuses(
    Vec<(
        Option<uuid::Uuid>,
        config::KeepPlants<config::ArchetypeHandle>,
        (config::PlantAdvancement, config::PlantAdvancementKind),
    )>,
);
impl NeighborBonuses {
    pub fn bonuses_for_plant(
        self,
        tile_id: uuid::Uuid,
        ah: config::ArchetypeHandle,
    ) -> Vec<config::PlantAdvancement> {
        self.0
            .into_iter()
            // neighbor bonuses apply to plants with matching archetype handles
            // coming from different tiles, if the tile is known.
            // if the tile isn't known, the bonus will still apply if the archetype
            // handle matches.
            .filter(|(from, keep_plants, _)| {
                keep_plants.allows(&ah)
                    && match from {
                        Some(f) => *f != tile_id,
                        None => true,
                    }
            })
            .map(|(_, _, (bonus, _))| bonus)
            .collect()
    }
}

#[derive(Clone, Debug)]
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

    pub fn neighbor_bonuses(&self) -> NeighborBonuses {
        use config::{PlantAdvancement, PlantAdvancementKind};
        use PlantAdvancementKind::*;

        fn unsheath_neighbor(
            adv: &PlantAdvancement,
        ) -> Option<(PlantAdvancement, PlantAdvancementKind)> {
            match &adv.kind {
                Neighbor(a) => Some((adv.clone(), *a.clone())),
                _ => None,
            }
        }

        NeighborBonuses(
            self.land
                .iter()
                .filter_map(|tile| Some((tile.id, tile.plant.as_ref()?)))
                .flat_map(|(steader, plant)| {
                    plant
                        .unlocked_advancements(std::iter::empty())
                        .filter_map(move |adv| {
                            Some((
                                Some(steader.clone()),
                                config::KeepPlants::Only(vec![plant.archetype_handle]),
                                unsheath_neighbor(adv)?,
                            ))
                        })
                })
                .chain(self.gotchis.iter().flat_map(|g| {
                    g.inner.plant_effects.iter().map(|spa| {
                        (
                            None,
                            spa.keep_plants.lookup_handles().unwrap(),
                            (spa.advancement.clone(), spa.advancement.kind.clone()),
                        )
                    })
                }))
                .chain(
                    self.inventory
                        .iter()
                        .filter_map(|i| Some(i.kind.keepsake()?.plant_effects.as_ref()))
                        .flat_map(|plant_effects: &Vec<_>| {
                            plant_effects.iter().map(|spa| {
                                (
                                    None,
                                    spa.keep_plants.lookup_handles().unwrap(),
                                    (spa.advancement.clone(), spa.advancement.kind.clone()),
                                )
                            })
                        }),
                )
                .collect(),
        )
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

    pub async fn spawn_possession(
        db: &DynamoDbClient,
        receiver: String,
        archetype_handle: ArchetypeHandle,
    ) -> Result<(), RusotoError<PutItemError>> {
        Hacksteader::give_possession(
            db,
            receiver.clone(),
            &Possession::new(
                archetype_handle,
                possess::Owner {
                    id: receiver.clone(),
                    acquisition: possess::Acquisition::spawned(),
                },
            ),
        )
        .await
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
