use humantime::{format_rfc3339, parse_rfc3339};
use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;

type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hackagotchi";

#[derive(serde::Deserialize)]
pub struct Config {
    pub special_users: Vec<String>,
    pub archetypes: Vec<Archetype>,
}
pub type ArchetypeHandle = usize;

#[derive(Deserialize, Debug)]
pub struct GotchiArchetype {
    pub base_happiness: usize,
}
#[derive(Deserialize, Debug)]
pub struct SeedArchetype;
#[derive(Deserialize, Debug)]
pub struct KeepsakeArchetype;

#[derive(Deserialize, Debug)]
pub enum ArchetypeKind {
    Gotchi(GotchiArchetype),
    Seed(SeedArchetype),
    Keepsake(KeepsakeArchetype),
}
#[derive(Deserialize, Debug)]
pub struct Archetype {
    pub name: String,
    pub kind: ArchetypeKind,
}

lazy_static::lazy_static! {
    pub static ref CONFIG: Config = {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/gotchi_config.json");
        let file = std::fs::File::open(path)
            .unwrap_or_else(|e| panic!("Couldn't open {}: {}", path, e));

        serde_json::from_reader(std::io::BufReader::new(file))
            .unwrap_or_else(|e| panic!("Couldn't parse {}: {}", path, e))
    };
}

pub struct Hacksteader {
    pub user_id: String,
    pub profile: HacksteaderProfile,
    pub gotchis: Vec<Possessed<Gotchi>>,
    pub seeds: Vec<Possessed<Seed>>,
    pub keepsakes: Vec<Possessed<Keepsake>>,
}
impl Hacksteader {
    pub async fn new_in_db(
        db: &DynamoDbClient,
        user_id: String,
    ) -> Result<(), RusotoError<PutItemError>> {
        // just give them a profile for now
        db.put_item(rusoto_dynamodb::PutItemInput {
            item: HacksteaderProfile::new().item(user_id),
            table_name: TABLE_NAME.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
    }

    pub async fn give_possession_from_archetype(
        db: &DynamoDbClient,
        user_id: String,
        ah: ArchetypeHandle,
    ) -> Result<(), RusotoError<PutItemError>> {
        let owner = Owner {
            id: user_id.clone(),
            acquisition: Acquisition::spawned(),
        };

        // eventually you probably want to be dynamic over the type at runtime but this works for now
        match CONFIG
            .archetypes
            .get(ah)
            .expect("invalid archetype handle")
            .kind
        {
            ArchetypeKind::Gotchi(_) => {
                let i = Possessed::<Gotchi>::new(ah, owner);
                Self::give_possession(db, user_id, i).await
            }
            ArchetypeKind::Seed(_) => {
                let i = Possessed::<Seed>::new(ah, owner);
                Self::give_possession(db, user_id, i).await
            }
            ArchetypeKind::Keepsake(_) => {
                let i = Possessed::<Keepsake>::new(ah, owner);
                Self::give_possession(db, user_id, i).await
            }
        }
    }

    pub async fn give_possession<P: Possessable>(
        db: &DynamoDbClient,
        user_id: String,
        possession: Possessed<P>,
    ) -> Result<(), RusotoError<PutItemError>> {
        db.put_item(rusoto_dynamodb::PutItemInput {
            item: possession.item(user_id),
            table_name: TABLE_NAME.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
    }

    pub async fn from_db(db: &DynamoDbClient, user_id: String) -> Option<Self> {
        let query = db
            .query(rusoto_dynamodb::QueryInput {
                table_name: TABLE_NAME.to_string(),
                key_condition_expression: Some("id = :db_id".to_string()),
                expression_attribute_values: Some({
                    let mut m = HashMap::new();
                    m.insert(
                        ":db_id".to_string(),
                        AttributeValue {
                            s: Some(user_id.clone()),
                            ..Default::default()
                        },
                    );
                    m
                }),
                ..Default::default()
            })
            .await;
        let items = query.ok()?.items?;

        let mut profile = None;
        let mut gotchis = Vec::new();
        let mut seeds = Vec::new();
        let mut keepsakes = Vec::new();

        for item in items.iter() {
            (|| -> Option<()> {
                let sk = item.get("sk")?.s.as_ref()?;
                match sk.chars().next()? {
                    'P' => profile = Some(HacksteaderProfile::from_item(item)?),
                    'G' => gotchis.push(Possessed::from_item(item)?),
                    'S' => seeds.push(Possessed::from_item(item)?),
                    'K' => keepsakes.push(Possessed::from_item(item)?),
                    _ => unreachable!(),
                }
                Some(())
            })();
        }

        Some(Hacksteader {
            profile: profile?,
            user_id,
            gotchis,
            seeds,
            keepsakes,
        })
    }
}

pub trait Possessable: std::ops::Deref + std::fmt::Debug + PartialEq + Clone + Sized {
    /// The archetype that corresponds to this Possessable
    type A;

    /// A char used in the ids that this Possessable serializes into in the database.
    const SIGN: char;

    fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self;
    fn archetype_handle(&self) -> ArchetypeHandle;
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A>;
    fn fill_from_item(&mut self, item: &Item) -> Option<()>;
    fn write_item(&self, item: &mut Item);
}

macro_rules! archetype_deref {
    ( $p:ident ) => {
        impl std::ops::Deref for $p {
            type Target = <Self as Possessable>::A;

            fn deref(&self) -> &Self::Target {
                Self::archetype_kind(
                    &CONFIG
                        .archetypes
                        .get(self.archetype_handle())
                        .expect("invalid archetype handle")
                        .kind,
                )
                .expect("archetype kind did not match instance kind")
            }
        }
    };
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Gotchi {
    archetype_handle: ArchetypeHandle,
    pub nickname: String,
    pub harvest_log: Vec<GotchiHarvestOwner>,
}
archetype_deref!(Gotchi);
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct GotchiHarvestOwner {
    pub id: String,
    pub harvested: usize,
}
impl GotchiHarvestOwner {
    fn from_item(item: &Item) -> Option<Self> {
        Some(Self {
            id: item.get("id")?.s.as_ref()?.clone(),
            harvested: item.get("harvested")?.n.as_ref()?.parse().ok()?,
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
impl Possessable for Gotchi {
    type A = GotchiArchetype;
    const SIGN: char = 'G';
    fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            nickname: CONFIG.archetypes[archetype_handle].name.clone(),
            harvest_log: vec![GotchiHarvestOwner {
                id: owner_id.to_string(),
                harvested: 0,
            }],
        }
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Gotchi(ga) => Some(ga),
            _ => None,
        }
    }
    fn archetype_handle(&self) -> ArchetypeHandle {
        self.archetype_handle
    }
    fn fill_from_item(&mut self, item: &Item) -> Option<()> {
        self.nickname = item.get("nickname")?.s.as_ref()?.clone();
        self.harvest_log = item
            .get("harvest_log")?
            .l
            .as_ref()?
            .iter()
            .filter_map(|v| GotchiHarvestOwner::from_item(v.m.as_ref()?))
            .collect();

        Some(())
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
#[test]
fn gotchi_serialize() {
    dotenv::dotenv().ok();

    let og = Gotchi::new(
        CONFIG
            .archetypes
            .iter()
            .position(|x| x.name == "Adorpheus")
            .unwrap(),
        "bob",
    );

    let mut og_item = Item::new();
    og.write_item(&mut og_item);

    let mut og_copy = Gotchi::default();
    og_copy.fill_from_item(&og_item);

    assert_eq!(og, og_copy);
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Seed {
    archetype_handle: ArchetypeHandle,
    pedigree: Vec<SeedGrower>,
}
archetype_deref!(Seed);
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct SeedGrower {
    id: String,
    generations: usize,
}
impl SeedGrower {
    fn from_item(item: &Item) -> Option<Self> {
        Some(Self {
            id: item.get("id")?.s.as_ref()?.clone(),
            generations: item.get("generations")?.n.as_ref()?.parse().ok()?,
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
impl Possessable for Seed {
    type A = SeedArchetype;
    const SIGN: char = 'G';
    fn new(archetype_handle: ArchetypeHandle, owner_id: &str) -> Self {
        Self {
            archetype_handle,
            pedigree: vec![SeedGrower {
                id: owner_id.to_string(),
                generations: 0,
            }],
        }
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Seed(sa) => Some(sa),
            _ => None,
        }
    }
    fn archetype_handle(&self) -> ArchetypeHandle {
        self.archetype_handle
    }
    fn fill_from_item(&mut self, item: &Item) -> Option<()> {
        self.pedigree = item
            .get("pedigree")?
            .l
            .as_ref()?
            .iter()
            .filter_map(|v| SeedGrower::from_item(v.m.as_ref()?))
            .collect();

        Some(())
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Keepsake {
    archetype_handle: ArchetypeHandle,
}
archetype_deref!(Keepsake);
impl Possessable for Keepsake {
    type A = KeepsakeArchetype;
    const SIGN: char = 'K';
    fn new(archetype_handle: ArchetypeHandle, _owner_id: &str) -> Self {
        Self { archetype_handle }
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Keepsake(ka) => Some(ka),
            _ => None,
        }
    }
    fn archetype_handle(&self) -> ArchetypeHandle {
        self.archetype_handle
    }
    fn fill_from_item(&mut self, _item: &Item) -> Option<()> {
        Some(())
    }
    fn write_item(&self, _item: &mut Item) {}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Owner {
    pub id: String,
    pub acquisition: Acquisition,
}
impl Owner {
    fn from_item(item: &Item) -> Option<Self> {
        Some(Self {
            id: item.get("id")?.s.as_ref()?.to_string(),
            acquisition: Acquisition::from_item(item.get("acquisition")?.m.as_ref()?)?,
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
    Purchase { price: usize },
}
impl Acquisition {
    fn from_item(item: &Item) -> Option<Self> {
        let kind = item.get("type")?.s.as_ref()?.to_string();
        match kind.as_str() {
            "Trade" => Some(Acquisition::Trade),
            "Purchase" => Some(Acquisition::Purchase {
                price: item.get("price")?.n.as_ref()?.parse().ok()?,
            }),
            _ => {
                println!("unknown Acquisition type");
                None
            }
        }
    }
    fn spawned() -> Self {
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

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub struct Possessed<P: Possessable> {
    pub inner: P,
    pub archetype_handle: ArchetypeHandle,
    id: uuid::Uuid,
    pub ownership_log: Vec<Owner>,
}

impl<P: Possessable> std::ops::Deref for Possessed<P> {
    type Target = Archetype;

    fn deref(&self) -> &Self::Target {
        self.archetype()
    }
}

impl<P: Possessable> Possessed<P> {
    pub fn new(archetype_handle: ArchetypeHandle, owner: Owner) -> Self {
        Self {
            inner: P::new(archetype_handle, &owner.id),
            id: uuid::Uuid::new_v4(),
            archetype_handle,
            ownership_log: vec![owner],
        }
    }

    fn archetype(&self) -> &Archetype {
        CONFIG
            .archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
    }

    pub fn sk(&self) -> String {
        let id_string = self.id.to_string();
        let mut sk = String::with_capacity(id_string.len() + self.name.len() + 3);
        sk.push(P::SIGN);
        sk.push('#');
        sk.push_str(&self.name);
        sk.push('#');
        sk.push_str(&id_string);
        sk
    }

    pub fn empty_item_from_parts(sk: String, slack_id: String) -> Item {
        let mut m = Item::new();
        m.insert(
            "id".to_string(),
            AttributeValue {
                s: Some(slack_id),
                ..Default::default()
            },
        );
        m.insert(
            "sk".to_string(),
            AttributeValue {
                s: Some(sk),
                ..Default::default()
            },
        );
        m
    }

    pub fn empty_item(&self, slack_id: String) -> Item {
        Self::empty_item_from_parts(self.sk(), slack_id)
    }

    fn item(&self, slack_id: String) -> Item {
        let mut m = self.empty_item(slack_id);
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
        self.inner.write_item(&mut m);
        m
    }

    pub fn from_item(item: &Item) -> Option<Self> {
        let mut sk_parts = item.get("sk")?.s.as_ref()?.split("#");
        let mut sign_section_chars = sk_parts.next()?.chars();

        // first section should have one char, that char should be the sign
        // of this possessable.
        assert_eq!(sign_section_chars.next(), Some(P::SIGN));
        assert_eq!(sign_section_chars.next(), None);

        let _name = sk_parts.next()?;
        let archetype_handle = item.get("archetype_handle")?.n.as_ref()?.parse().ok()?;

        let mut inner = P::new(archetype_handle, item.get("id")?.s.as_ref()?);
        inner.fill_from_item(item);

        Some(Self {
            inner,
            archetype_handle,
            id: uuid::Uuid::parse_str(sk_parts.next()?).ok()?,
            ownership_log: item
                .get("ownership_log")?
                .l
                .as_ref()?
                .iter()
                .map(|x| Owner::from_item(x.m.as_ref()?))
                .collect::<Option<_>>()?,
        })
    }
}
#[test]
fn possessed_gotchi_serialize() {
    dotenv::dotenv().ok();

    let og = Possessed::<Gotchi>::new(
        CONFIG
            .archetypes
            .iter()
            .position(|x| x.name == "Adorpheus")
            .unwrap(),
        Owner {
            id: "bob".to_string(),
            acquisition: Acquisition::spawned(),
        },
    );

    let og_item = og.item("bob".to_string());

    assert_eq!(og, Possessed::<Gotchi>::from_item(&og_item).unwrap());
}

pub struct HacksteaderProfile {
    /// Indicates when this Hacksteader first joined the elite community.
    pub joined: SystemTime,
}
impl HacksteaderProfile {
    pub fn new() -> Self {
        Self {
            joined: SystemTime::now(),
        }
    }

    /// Returns an empty profile Item for the given slack ID.
    /// Useful for searching for a given slack user's Hacksteader profile
    fn empty_item(user_id: String) -> Item {
        let mut m = Item::new();
        m.insert(
            "id".to_string(),
            AttributeValue {
                s: Some(user_id),
                ..Default::default()
            },
        );
        m.insert(
            "sk".to_string(),
            AttributeValue {
                s: Some("P".to_string()),
                ..Default::default()
            },
        );
        m
    }

    pub fn item(&self, user_id: String) -> Item {
        let mut m = Self::empty_item(user_id.clone());
        m.insert(
            "joined".to_string(),
            AttributeValue {
                s: Some(format_rfc3339(self.joined).to_string()),
                ..Default::default()
            },
        );
        m
    }

    pub fn from_item(item: &Item) -> Option<Self> {
        Some(Self {
            joined: item
                .get("joined")
                .and_then(|a| a.s.as_ref())
                .and_then(|s| parse_rfc3339(s).ok())
                .unwrap_or_else(|| SystemTime::now()),
        })
    }
}
