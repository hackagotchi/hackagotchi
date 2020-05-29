use rusoto_dynamodb::AttributeValue;
use std::fmt;
use serde::{Serialize, Deserialize};
use crate::{market, Key, AttributeParseError, config, CONFIG, Item, Category};
use config::{ArchetypeHandle, Archetype, ArchetypeKind};

mod keepsake;
pub mod gotchi;
pub mod seed;

pub use keepsake::Keepsake;
pub use gotchi::Gotchi;
pub use seed::Seed;

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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Owner {
    pub id: String,
    pub acquisition: Acquisition,
}
impl Owner {
    pub fn farmer(id: String) -> Self {
        Self {
            id,
            acquisition: Acquisition::Farmed,
        }
    }
    pub fn crafter(id: String) -> Self {
        Self {
            id,
            acquisition: Acquisition::Crafted,
        }
    }
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
    Farmed,
    Crafted,
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
            "Farmed" => Ok(Acquisition::Farmed),
            "Crafted" => Ok(Acquisition::Crafted),
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
            Acquisition::Farmed => write!(f, "Farmed"),
            Acquisition::Crafted => write!(f, "Crafted"),
            Acquisition::Purchase { price } => write!(f, "Purchase({}gp)", price),
        }
    }
}
impl Into<AttributeValue> for Acquisition {
    fn into(self) -> AttributeValue {
        AttributeValue {
            m: match self {
                Acquisition::Trade | Acquisition::Farmed | Acquisition::Crafted => Some(
                    [(
                        "type".to_string(),
                        AttributeValue {
                            s: Some(format!("{}", self)),
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

    pub fn item(&self) -> Item {
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
