#![feature(try_trait)]

use rusoto_dynamodb::{DynamoDb, DynamoDbClient, AttributeValue};
use humantime::{format_rfc3339, parse_rfc3339};
use std::collections::HashMap;
use std::time::SystemTime;
use std::fmt;

pub mod market;
pub mod config;
pub mod possess;
pub mod category;

pub use category::{Category, CategoryError};
pub use possess::{Possession, Possessed};
pub use config::CONFIG;

pub const TABLE_NAME: &'static str = "hackagotchi";
pub type Item = HashMap<String, AttributeValue>;

#[derive(Clone, Debug, PartialEq)]
pub enum AttributeParseError {
    IntFieldParse(&'static str, std::num::ParseIntError),
    FloatFieldParse(&'static str, std::num::ParseFloatError),
    TimeFieldParse(&'static str, humantime::TimestampError),
    IdFieldParse(&'static str, uuid::Error),
    CategoryParse(CategoryError),
    MissingField(&'static str),
    WronglyTypedField(&'static str),
    WrongType,
    Unknown,
    Custom(&'static str),
}
impl Into<String> for AttributeParseError {
    fn into(self) -> String {
        format!("{}", self)
    }
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
            FloatFieldParse(field, e) => write!(f, "error parsing float field {:?}: {}", field, e),
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


#[derive(Clone)]
pub struct Profile {
    /// Indicates when this Hacksteader first joined the elite community.
    pub joined: SystemTime,
    pub last_active: SystemTime,
    pub last_farm: SystemTime,
    /// This is not an uuid::Uuid because it's actually the steader id of the person who owns this Profile
    pub id: String,
    pub xp: u64,
}

impl std::ops::Deref for Profile {
    type Target = config::ProfileArchetype;

    fn deref(&self) -> &Self::Target {
        &CONFIG.profile_archetype
    }
}

impl Profile {
    pub fn new(owner_id: String) -> Self {
        Self {
            joined: SystemTime::now(),
            last_active: SystemTime::now(),
            last_farm: SystemTime::now(),
            xp: 0,
            id: owner_id,
        }
    }

    // TODO: store xp in advancements so methods like these aren't necessary
    pub fn current_advancement(&self) -> &config::HacksteadAdvancement {
        self.advancements.current(self.xp)
    }

    pub fn next_advancement(&self) -> Option<&config::HacksteadAdvancement> {
        self.advancements.next(self.xp)
    }

    pub fn advancements_sum(&self) -> config::HacksteadAdvancementSum {
        self.advancements.sum(self.xp, std::iter::empty())
    }

    pub fn increment_xp(&mut self) -> Option<&config::HacksteadAdvancement> {
        CONFIG
            .profile_archetype
            .advancements
            .increment_xp(&mut self.xp)
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
            "last_active".to_string(),
            AttributeValue {
                s: Some(format_rfc3339(self.last_active).to_string()),
                ..Default::default()
            },
        );
        m.insert(
            "last_farm".to_string(),
            AttributeValue {
                s: Some(format_rfc3339(self.last_farm).to_string()),
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
                Some(xp_attribtue_value) => xp_attribtue_value
                    .n
                    .as_ref()
                    .ok_or(WronglyTypedField("xp"))?
                    .parse()
                    .map_err(|x| IntFieldParse("xp", x))?,
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
            last_active: parse_rfc3339(
                item.get("last_active")
                    .ok_or(MissingField("last_active"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("last_active"))?,
            )
            .map_err(|e| TimeFieldParse("last_active", e))?,
            last_farm: parse_rfc3339(
                item.get("last_farm")
                    .ok_or(MissingField("last_farm"))?
                    .s
                    .as_ref()
                    .ok_or(WronglyTypedField("last_farm"))?,
            )
            .map_err(|e| TimeFieldParse("last_farm", e))?,
        })
    }
}

/// A model for all keys that use uuid:Uuids internally,
/// essentially all those except Profile keys.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Copy)]
pub struct Key {
    pub category: Category,
    pub id: uuid::Uuid,
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

    pub async fn fetch_db(self, db: &DynamoDbClient) -> Result<Possession, String> {
        match db
            .get_item(rusoto_dynamodb::GetItemInput {
                key: self.clone().into_item(),
                table_name: TABLE_NAME.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(o) => {
                Possession::from_item(&o.item.ok_or_else(|| format!("key[{:?}] not in db", self))?)
                    .map_err(|e| format!("couldn't parse item: {}", e))
            }
            Err(e) => Err(format!("Couldn't read key[{:?}] from db: {}", self, e)),
        }
    }
}
