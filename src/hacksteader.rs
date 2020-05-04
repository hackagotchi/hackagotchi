use super::market;
use super::config::{self, CONFIG, ArchetypeKind, Archetype, ArchetypeHandle, PlantArchetype};
use humantime::{format_rfc3339, parse_rfc3339};
use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt;
use std::time::SystemTime;

pub type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hackagotchi";

// A searchable category in the market. May or may not
// correspond 1:1 to an Archetype.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum Category {
    Profile = 0,
    Gotchi = 1,
    Misc = 2,
    Land = 3,
    Sale = 9,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CategoryError {
    UnknownCategory,
    InvalidAttributeValue,
    InvalidNumber(std::num::ParseIntError),
}
impl fmt::Display for CategoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CategoryError::*;

        match self {
            UnknownCategory => write!(f, "Unknown Category!"),
            InvalidAttributeValue => write!(f, "Category AttributeValue wasn't a number!"),
            InvalidNumber(e) => {
                write!(f, "Couldn't parse number in Category AttributeValue: {}", e)
            }
        }
    }
}
impl From<std::num::ParseIntError> for CategoryError {
    fn from(o: std::num::ParseIntError) -> Self {
        CategoryError::InvalidNumber(o)
    }
}

impl std::convert::TryFrom<u8> for Category {
    type Error = CategoryError;

    fn try_from(o: u8) -> Result<Self, Self::Error> {
        use Category::*;

        Ok(match o {
            0 => Profile,
            1 => Gotchi,
            2 => Misc,
            3 => Land,
            9 => Sale,
            _ => return Err(CategoryError::UnknownCategory),
        })
    }
}

impl Category {
    /*
    fn iter() -> impl ExactSizeIterator<Item = Category> {
        use Category::*;
        [Profile, Gotchi, Misc, Sale].iter().cloned()
    }*/

    pub fn from_av(av: &AttributeValue) -> Result<Self, CategoryError> {
        av.n.as_ref()
            .ok_or(CategoryError::InvalidAttributeValue)?
            .parse::<u8>()?
            .try_into()
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            n: Some((self as u8).to_string()),
            ..Default::default()
        }
    }
}

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

#[derive(Clone, Debug, PartialEq)]
pub enum AttributeParseError {
    IntFieldParse(&'static str, std::num::ParseIntError),
    TimeFieldParse(&'static str, humantime::TimestampError),
    IdFieldParse(&'static str, uuid::Error),
    CategoryParse(CategoryError),
    MissingField(&'static str),
    WronglyTypedField(&'static str),
    WrongType,
    Unknown,
    Custom(&'static str),
}

impl From<CategoryError> for AttributeParseError {
    fn from(o: CategoryError) -> Self {
        AttributeParseError::CategoryParse(o)
    }
}
impl From<std::option::NoneError> for AttributeParseError {
    fn from(_: std::option::NoneError) -> Self {
        AttributeParseError::Unknown
    }
}

impl fmt::Display for AttributeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use AttributeParseError::*;
        match self {
            IntFieldParse(field, e) => write!(f, "error parsing integer field {:?}: {}", field, e),
            TimeFieldParse(field, e) => {
                write!(f, "error parsing timestamp field {:?}: {}", field, e)
            }
            IdFieldParse(field, e) => write!(f, "error parsing id field {:?}: {}", field, e),
            MissingField(field) => write!(f, "missing field {:?}", field),
            CategoryParse(e) => write!(f, "failed parsing category {}", e),
            WronglyTypedField(field) => write!(f, "wrongly typed field {:?}", field),
            WrongType => write!(f, "wrong AttributeValue type"),
            Unknown => write!(f, "unknown parsing error"),
            Custom(e) => write!(f, "{}", e),
        }
    }
}

pub async fn landfill(db: &DynamoDbClient) -> Result<(), String> {
    let tiles = Tile::fetch_all(db).await?;

    db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
        request_items: [(
            TABLE_NAME.to_string(),
            tiles
                .into_iter()
                .map(|mut tile| rusoto_dynamodb::WriteRequest {
                    put_request: Some(rusoto_dynamodb::PutRequest {
                        item: {
                            tile.plant.take();

                            tile
                                .into_av()
                                .m
                                .expect("tile attribute should be map")
                        }
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

#[derive(Debug)]
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

    pub async fn plant(
        db: &DynamoDbClient,
        tile_id: uuid::Uuid,
        seed_ah: ArchetypeHandle,
    ) -> Result<(), String> {
        match db
            .update_item(rusoto_dynamodb::UpdateItemInput {
                table_name: TABLE_NAME.to_string(),
                key: Key::tile(tile_id).into_item(),
                update_expression: Some("SET plant = :baby_plant".to_string()),
                expression_attribute_values: Some(
                    [(":baby_plant".to_string(), Plant::new(seed_ah).into_av())]
                        .iter()
                        .cloned()
                        .collect(),
                ),
                ..Default::default()
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("couldn't plant on tile in db: {}", e)),
        }
    }

    pub fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        Ok(Self {
            acquired: parse_rfc3339(
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
                        s: Some(format_rfc3339(self.acquired).to_string()),
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

#[derive(Debug)]
pub struct Plant {
    pub xp: u64,
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

impl Plant {
    pub fn new(ah: ArchetypeHandle) -> Self {
        Self {
            xp: 0,
            archetype_handle: ah,
        }
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
            archetype_handle: m
                .get("archetype_handle")
                .ok_or(MissingField("archetype_handle"))?
                .n
                .as_ref()
                .ok_or(WronglyTypedField("archetype_handle"))?
                .parse()
                .map_err(|e| IntFieldParse("archetype_handle", e))?,
        })
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            m: Some(
                [
                    (
                        "xp".to_string(),
                        AttributeValue {
                            n: Some(self.xp.to_string()),
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
                ]
                .iter()
                .cloned()
                .collect(),
            ),
            ..Default::default()
        }
    }
}

/// A model for all keys that use uuid:Uuids internally,
/// essentially all those except Profile keys.
pub struct Key {
    category: Category,
    id: uuid::Uuid,
}
impl Key {
    #[allow(dead_code)]
    pub fn gotchi(id: uuid::Uuid) -> Self {
        Self {
            category: Category::Gotchi,
            id,
        }
    }
    pub fn misc(id: uuid::Uuid) -> Self {
        Self {
            category: Category::Misc,
            id,
        }
    }
    pub fn tile(id: uuid::Uuid) -> Self {
        Self {
            category: Category::Land,
            id,
        }
    }

    pub fn into_item(self) -> Item {
        [
            ("cat".to_string(), self.category.into_av()),
            (
                "id".to_string(),
                AttributeValue {
                    s: Some(self.id.to_string()),
                    ..Default::default()
                },
            ),
        ]
        .iter()
        .cloned()
        .collect()
    }

    pub fn from_item(i: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        Ok(Self {
            category: Category::from_av(i.get("cat").ok_or(MissingField("cat"))?)?,
            id: uuid::Uuid::parse_str(
                i.get("id")
                    .ok_or(MissingField("id"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("id"))?,
            )
            .map_err(|e| IdFieldParse("id", e))?,
        })
    }
}

pub struct Hacksteader {
    pub user_id: String,
    pub profile: Profile,
    pub land: Vec<Tile>,
    pub inventory: Vec<Possession>,
    pub gotchis: Vec<Possessed<Gotchi>>,
}
impl Hacksteader {
    pub async fn new_in_db(
        db: &DynamoDbClient,
        user_id: String,
    ) -> Result<(), String> {
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

    pub async fn transfer_possession(
        db: &DynamoDbClient,
        new_owner: String,
        acquisition: Acquisition,
        possession: &Possession,
    ) -> Result<(), String> {
        db.update_item(rusoto_dynamodb::UpdateItemInput {
            table_name: TABLE_NAME.to_string(),
            key: possession.key().into_item(),
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
                            l: Some(vec![Owner {
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

    pub async fn from_db(db: &DynamoDbClient, user_id: String) -> Option<Self> {
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
            .map_err(|e| println!("couldn't profile query: {}", e))
            .ok()?
            .items?;

        let mut profile = None;
        let mut gotchis = Vec::new();
        let mut inventory = Vec::new();
        let mut land = Vec::new();

        for item in items.iter() {
            match (|| -> Result<(), AttributeParseError> {
                use AttributeParseError::*;

                match dbg!(Category::from_av(
                    item.get("cat").ok_or(MissingField("cat"))?
                )?) {
                    Category::Profile => profile = Some(Profile::from_item(item)?),
                    Category::Gotchi => {
                        gotchis.push(Possessed::from_possession(Possession::from_item(item)?)?)
                    }
                    Category::Misc => inventory.push(Possession::from_item(item)?),
                    Category::Land => land.push(dbg!(Tile::from_item(item)
                        .map_err(|e| println!("tile parse err: {}", e))
                        .ok()?)),
                    _ => unreachable!(),
                }
                Ok(())
            })() {
                Err(e) => println!("error parsing hackstead item: {}", e),
                Ok(()) => {}
            }
        }

        Some(Hacksteader {
            profile: profile?,
            user_id,
            gotchis,
            inventory,
            land,
        })
    }
}

pub trait Possessable: Sized {
    fn from_possession_kind(pk: PossessionKind) -> Option<Self>;
    fn into_possession_kind(self) -> PossessionKind;
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub enum PossessionKind {
    Gotchi(Gotchi),
    Seed(Seed),
    Keepsake(Keepsake),
}
impl PossessionKind {
    pub fn category(&self) -> Category {
        match self {
            PossessionKind::Gotchi(_) => Category::Gotchi,
            _ => Category::Misc,
        }
    }
    fn new(ah: ArchetypeHandle, owner_id: &str) -> Self {
        match CONFIG
            .possession_archetypes
            .get(ah)
            .unwrap_or_else(|| panic!("Unknown archetype: {}", ah))
            .kind
        {
            ArchetypeKind::Gotchi(_) => PossessionKind::Gotchi(Gotchi::new(ah, owner_id)),
            ArchetypeKind::Seed(_) => PossessionKind::Seed(Seed::new(ah, owner_id)),
            ArchetypeKind::Keepsake(_) => PossessionKind::Keepsake(Keepsake::new(ah, owner_id)),
        }
    }
    fn fill_from_item(&mut self, item: &Item) -> Result<(), AttributeParseError> {
        match self {
            PossessionKind::Gotchi(g) => g.fill_from_item(item),
            PossessionKind::Seed(s) => s.fill_from_item(item),
            PossessionKind::Keepsake(k) => k.fill_from_item(item),
        }
    }
    fn write_item(&self, item: &mut Item) {
        match self {
            PossessionKind::Gotchi(g) => g.write_item(item),
            PossessionKind::Seed(s) => s.write_item(item),
            PossessionKind::Keepsake(k) => k.write_item(item),
        }
    }

    pub fn as_gotchi(self) -> Option<Gotchi> {
        match self {
            PossessionKind::Gotchi(g) => Some(g),
            _ => None,
        }
    }
    pub fn gotchi(&self) -> Option<&Gotchi> {
        match self {
            PossessionKind::Gotchi(g) => Some(g),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn is_gotchi(&self) -> bool {
        match self {
            PossessionKind::Gotchi(_) => true,
            _ => false,
        }
    }
    pub fn gotchi_mut(&mut self) -> Option<&mut Gotchi> {
        match self {
            PossessionKind::Gotchi(g) => Some(g),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_seed(self) -> Option<Seed> {
        match self {
            PossessionKind::Seed(g) => Some(g),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn seed(&self) -> Option<&Seed> {
        match self {
            PossessionKind::Seed(g) => Some(g),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn is_seed(&self) -> bool {
        match self {
            PossessionKind::Seed(_) => true,
            _ => false,
        }
    }
    #[allow(dead_code)]
    pub fn seed_mut(&mut self) -> Option<&mut Seed> {
        match self {
            PossessionKind::Seed(g) => Some(g),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_keepsake(self) -> Option<Keepsake> {
        match self {
            PossessionKind::Keepsake(g) => Some(g),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn keepsake(&self) -> Option<&Keepsake> {
        match self {
            PossessionKind::Keepsake(g) => Some(g),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn is_keepsake(&self) -> bool {
        match self {
            PossessionKind::Keepsake(_) => true,
            _ => false,
        }
    }
    #[allow(dead_code)]
    pub fn keepsake_mut(&mut self) -> Option<&mut Keepsake> {
        match self {
            PossessionKind::Keepsake(g) => Some(g),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct GotchiHarvestOwner {
    pub id: String,
    pub harvested: u64,
}
impl GotchiHarvestOwner {
    fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
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
    fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            nickname: CONFIG.possession_archetypes[archetype_handle].name.clone(),
            harvest_log: vec![GotchiHarvestOwner {
                id: owner_id.to_string(),
                harvested: 0,
            }],
        }
    }
    fn fill_from_item(&mut self, item: &Item) -> Result<(), AttributeParseError> {
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
    fn write_item(&self, item: &mut Item) {
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
    fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            pedigree: vec![SeedGrower {
                id: owner_id.to_string(),
                generations: 0,
            }],
        }
    }
    fn fill_from_item(&mut self, item: &Item) -> Result<(), AttributeParseError> {
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
    fn write_item(&self, item: &mut Item) {
        item.insert(
            "pedigree".to_string(),
            AttributeValue {
                l: Some(self.pedigree.iter().cloned().map(|sg| sg.into()).collect()),
                ..Default::default()
            },
        );
    }
}
impl Keepsake {
    fn new(archetype_handle: ArchetypeHandle, _owner_id: &str) -> Self {
        Self { archetype_handle }
    }
    fn fill_from_item(&mut self, _item: &Item) -> Result<(), AttributeParseError> {
        Ok(())
    }
    fn write_item(&self, _item: &mut Item) {}
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
    fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Keepsake {
    archetype_handle: ArchetypeHandle,
}
impl Possessable for Keepsake {
    fn from_possession_kind(pk: PossessionKind) -> Option<Self> {
        pk.as_keepsake()
    }
    fn into_possession_kind(self) -> PossessionKind {
        PossessionKind::Keepsake(self)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Owner {
    pub id: String,
    pub acquisition: Acquisition,
}
impl Owner {
    fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        Ok(Self {
            id: item
                .get("id")
                .ok_or(MissingField("id"))?
                .s
                .as_ref()
                .ok_or(WronglyTypedField("id"))?
                .clone(),
            acquisition: Acquisition::from_item(
                item.get("acquisition")
                    .ok_or(MissingField("acquisition"))?
                    .m
                    .as_ref()
                    .ok_or(WronglyTypedField("acquisition"))?,
            )?,
        })
    }
}
impl Into<AttributeValue> for Owner {
    fn into(self) -> AttributeValue {
        let Self { id, acquisition } = self;
        AttributeValue {
            m: Some(
                [
                    (
                        "id".into(),
                        AttributeValue {
                            s: Some(id),
                            ..Default::default()
                        },
                    ),
                    ("acquisition".into(), acquisition.into()),
                ]
                .iter()
                .cloned()
                .collect(),
            ),
            ..Default::default()
        }
    }
}
#[test]
fn owner_serialize() {
    dotenv::dotenv().ok();

    let og = Owner {
        id: "bob".to_string(),
        acquisition: Acquisition::spawned(),
    };

    let og_av: AttributeValue = og.clone().into();
    let item = &og_av.m.unwrap();
    let og_copy = Owner::from_item(item).unwrap();

    assert_eq!(og, og_copy);
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Acquisition {
    Trade,
    Purchase { price: u64 },
}
impl Acquisition {
    fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        let kind = item
            .get("type")
            .ok_or(MissingField("type"))?
            .s
            .as_ref()
            .ok_or(WronglyTypedField("type"))?
            .clone();
        match kind.as_str() {
            "Trade" => Ok(Acquisition::Trade),
            "Purchase" => Ok(Acquisition::Purchase {
                price: item
                    .get("price")
                    .ok_or(MissingField("price"))?
                    .n
                    .as_ref()
                    .ok_or(WronglyTypedField("price"))?
                    .parse()
                    .map_err(|e| IntFieldParse("price", e))?,
            }),
            _ => Err(Custom("unknown Acquisition type")),
        }
    }
    pub fn spawned() -> Self {
        Acquisition::Trade
    }
}
impl fmt::Display for Acquisition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Acquisition::Trade => write!(f, "Trade"),
            Acquisition::Purchase { price } => write!(f, "Purchase({}gp)", price),
        }
    }
}
impl Into<AttributeValue> for Acquisition {
    fn into(self) -> AttributeValue {
        AttributeValue {
            m: match self {
                Acquisition::Trade => Some(
                    [(
                        "type".to_string(),
                        AttributeValue {
                            s: Some("Trade".to_string()),
                            ..Default::default()
                        },
                    )]
                    .iter()
                    .cloned()
                    .collect(),
                ),
                Acquisition::Purchase { price } => Some(
                    [
                        (
                            "type".to_string(),
                            AttributeValue {
                                s: Some("Purchase".to_string()),
                                ..Default::default()
                            },
                        ),
                        (
                            "price".to_string(),
                            AttributeValue {
                                n: Some(price.to_string()),
                                ..Default::default()
                            },
                        ),
                    ]
                    .iter()
                    .cloned()
                    .collect(),
                ),
            },
            ..Default::default()
        }
    }
}
#[test]
fn acquisition_serialize() {
    dotenv::dotenv().ok();

    let og = Acquisition::spawned();

    let og_av: AttributeValue = og.clone().into();
    let item = &og_av.m.unwrap();
    let og_copy = Acquisition::from_item(item).unwrap();

    assert_eq!(og, og_copy);
}

/// A copy of Possession for when you know what variant of PossessionKind
/// you have at compiletime and want to easily access its properties alongside
/// those properties all Possessions have.
#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub struct Possessed<P: Possessable> {
    pub inner: P,
    pub archetype_handle: ArchetypeHandle,
    pub id: uuid::Uuid,
    pub steader: String,
    pub ownership_log: Vec<Owner>,
    pub sale: Option<market::Sale>,
}
impl<P: Possessable> std::convert::TryFrom<Possession> for Possessed<P> {
    type Error = &'static str;

    fn try_from(p: Possession) -> Result<Self, Self::Error> {
        Possessed::from_possession(p).ok_or("wrongly typed possession")
    }
}
impl<P: Possessable> Possessed<P> {
    /// Use of the TryFrom implementation is preferred, but this
    /// static method is still exposed as a matter of convenience
    pub fn from_possession(p: Possession) -> Option<Possessed<P>> {
        let Possession {
            kind,
            archetype_handle,
            id,
            steader,
            ownership_log,
            sale,
        } = p;

        Some(Self {
            inner: P::from_possession_kind(kind)?,
            archetype_handle,
            id,
            steader,
            ownership_log,
            sale,
        })
    }
    pub fn into_possession(self) -> Possession {
        let Self {
            inner,
            archetype_handle,
            id,
            steader,
            ownership_log,
            sale,
        } = self;

        Possession {
            kind: P::into_possession_kind(inner),
            archetype_handle,
            id,
            steader,
            ownership_log,
            sale,
        }
    }
}

impl<P: Possessable> std::ops::Deref for Possessed<P> {
    type Target = Archetype;

    fn deref(&self) -> &Self::Target {
        self.archetype()
    }
}

impl<P: Possessable> Possessed<P> {
    fn archetype(&self) -> &Archetype {
        CONFIG
            .possession_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub struct Possession {
    pub kind: PossessionKind,
    pub archetype_handle: ArchetypeHandle,
    pub id: uuid::Uuid,
    pub steader: String,
    pub ownership_log: Vec<Owner>,
    pub sale: Option<market::Sale>,
}

impl std::ops::Deref for Possession {
    type Target = Archetype;

    fn deref(&self) -> &Self::Target {
        self.archetype()
    }
}

impl Possession {
    pub fn new(archetype_handle: ArchetypeHandle, owner: Owner) -> Self {
        Self {
            kind: PossessionKind::new(archetype_handle, &owner.id),
            id: uuid::Uuid::new_v4(),
            archetype_handle,
            steader: owner.id.clone(),
            ownership_log: vec![owner],
            sale: None,
        }
    }

    pub fn nickname(&self) -> &str {
        match self.kind {
            PossessionKind::Gotchi(ref g) => &g.nickname,
            _ => &self.name,
        }
    }

    fn archetype(&self) -> &Archetype {
        CONFIG
            .possession_archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
    }

    pub fn key(&self) -> Key {
        Key {
            id: self.id.clone(),
            category: self.kind.category(),
        }
    }

    fn item(&self) -> Item {
        let mut m = self.key().into_item();
        m.insert(
            "steader".to_string(),
            AttributeValue {
                s: Some(self.steader.clone()),
                ..Default::default()
            },
        );
        m.insert(
            "ownership_log".to_string(),
            AttributeValue {
                l: Some(
                    self.ownership_log
                        .clone()
                        .into_iter()
                        .map(|x| x.into())
                        .collect(),
                ),
                ..Default::default()
            },
        );
        m.insert(
            "archetype_handle".to_string(),
            AttributeValue {
                n: Some(self.archetype_handle.to_string()),
                ..Default::default()
            },
        );
        self.kind.write_item(&mut m);
        m
    }

    pub fn from_item(item: &Item) -> Result<Self, AttributeParseError> {
        use AttributeParseError::*;

        let steader = item
            .get("steader")
            .ok_or(MissingField("steader"))?
            .s
            .as_ref()
            .ok_or(WronglyTypedField("steader"))?
            .clone();
        let Key { id, category } = Key::from_item(item)?;

        // make sure this is the right category of item
        let archetype_handle = item
            .get("archetype_handle")
            .ok_or(MissingField("archetype_handle"))?
            .n
            .as_ref()
            .ok_or(WronglyTypedField("archetype_handle"))?
            .parse()
            .map_err(|e| IntFieldParse("archetype_handle", e))?;

        let mut kind = PossessionKind::new(archetype_handle, &steader);
        kind.fill_from_item(item)?;

        if category == kind.category() {
            Ok(Self {
                steader,
                kind,
                archetype_handle,
                id,
                ownership_log: item
                    .get("ownership_log")
                    .ok_or(MissingField("ownership_log"))?
                    .l
                    .as_ref()
                    .ok_or(WronglyTypedField("ownership_log"))?
                    .iter()
                    .filter_map(|x| {
                        match x.m.as_ref() {
                            Some(m) => match Owner::from_item(m) {
                                Ok(o) => return Some(o),
                                Err(e) => println!("error parsing item in ownership log: {}", e),
                            },
                            None => println!("non-map item in ownership log"),
                        };
                        None
                    })
                    .collect(),
                sale: market::Sale::from_item(item).ok(),
            })
        } else {
            Err(Custom("Category mismatch"))
        }
    }
}
#[test]
fn possessed_gotchi_serialize() {
    dotenv::dotenv().ok();

    let og = Possession::new(
        CONFIG
            .possession_archetypes
            .iter()
            .position(|x| x.name == "Adorpheus")
            .unwrap(),
        Owner {
            id: "bob".to_string(),
            acquisition: Acquisition::spawned(),
        },
    );

    let og_item = og.item();

    assert_eq!(og, Possession::from_item(&og_item).unwrap());
}

pub struct Profile {
    /// Indicates when this Hacksteader first joined the elite community.
    pub joined: SystemTime,
    /// This is not an uuid::Uuid because it's actually the steader id of the person who owns this Profile
    pub id: String,
    pub xp: u64
}
impl Profile {
    pub fn new(owner_id: String) -> Self {
        Self {
            joined: SystemTime::now(),
            xp: 0,
            id: owner_id,
        }
    }

    pub async fn fetch_all(db: &DynamoDbClient) -> Result<Vec<Profile>, String> {
        let query = db
            .query(rusoto_dynamodb::QueryInput {
                table_name: TABLE_NAME.to_string(),
                key_condition_expression: Some("cat = :profile_cat".to_string()),
                expression_attribute_values: Some(
                    [(":profile_cat".to_string(), Category::Profile.into_av())]
                        .iter()
                        .cloned()
                        .collect(),
                ),
                ..Default::default()
            })
            .await;

        Ok(query
            .map_err(|e| format!("Couldn't search land cat: {}", e))?
            .items
            .ok_or_else(|| format!("profile query returned no items"))?
            .iter_mut()
            .filter_map(|i| match Profile::from_item(i) {
                Ok(profile) => Some(profile),
                Err(e) => {
                    println!("error parsing profile: {}", e);
                    None
                }
            })
            .collect())
    }

    /// Returns an empty profile Item for the given slack ID.
    /// Useful for searching for a given slack user's Hacksteader profile
    fn key_item(id: String) -> Item {
        [
            ("cat".to_string(), Category::Profile.into_av()),
            (
                "id".to_string(),
                AttributeValue {
                    s: Some(id),
                    ..Default::default()
                },
            ),
        ]
        .iter()
        .cloned()
        .collect()
    }

    pub fn item(&self) -> Item {
        let mut m = Self::key_item(self.id.clone());
        m.insert(
            "steader".to_string(),
            AttributeValue {
                s: Some(self.id.clone()),
                ..Default::default()
            },
        );
        m.insert(
            "joined".to_string(),
            AttributeValue {
                s: Some(format_rfc3339(self.joined).to_string()),
                ..Default::default()
            },
        );
        m.insert(
            "xp".to_string(),
            AttributeValue {
                n: Some(self.xp.to_string()),
                ..Default::default()
            },
        );
        m
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
            xp: match item.get("xp") {
                Some(xp_attribtue_value) => {
                    xp_attribtue_value
                        .n
                        .as_ref()
                        .ok_or(WronglyTypedField("xp"))?
                        .parse()
                        .map_err(|x| IntFieldParse("xp", x))?
                }
                None => 0,
            },
            joined: parse_rfc3339(
                item.get("joined")
                    .ok_or(MissingField("joined"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("joined"))?,
            )
            .map_err(|e| TimeFieldParse("joined", e))?,
        })
    }
}
