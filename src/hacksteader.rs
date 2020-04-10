use humantime::{format_rfc3339, parse_rfc3339};
use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use std::collections::HashMap;
use std::time::SystemTime;

type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hackagotchi";

#[derive(serde::Deserialize)]
pub struct Config {
    pub special_users: Vec<String>,
    pub archetypes: Vec<Archetype>,
}
pub type ArchetypeHandle = usize;

#[derive(serde::Deserialize, Debug)]
pub struct GotchiArchetype {
    pub power: usize,
}
#[derive(serde::Deserialize, Debug)]
pub struct SeedArchetype;
#[derive(serde::Deserialize, Debug)]
pub struct KeepsakeArchetype;

#[derive(serde::Deserialize, Debug)]
pub enum ArchetypeKind {
    Gotchi(GotchiArchetype),
    Seed(SeedArchetype),
    Keepsake(KeepsakeArchetype),
}
#[derive(serde::Deserialize, Debug)]
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
        // eventually you probably want to be dynamic over the type at runtime but this works for now
        match CONFIG.archetypes.get(ah).expect("invalid archetype handle").kind {
            ArchetypeKind::Gotchi(_) => {
                let i = Possessed::<Gotchi>::new(ah, user_id.clone());
                Self::give_possession(db, user_id, i).await
            }
            ArchetypeKind::Seed(_) => {
                let i = Possessed::<Seed>::new(ah, user_id.clone());
                Self::give_possession(db, user_id, i).await
            }
            ArchetypeKind::Keepsake(_) => {
                let i = Possessed::<Keepsake>::new(ah, user_id.clone());
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

pub trait Possessable: std::ops::Deref + std::fmt::Debug + Sized {
    /// The archetype that corresponds to this Possessable
    type A;

    /// A char used in the ids that this Possessable serializes into in the database.
    const SIGN: char;

    fn new(archetype_handle: ArchetypeHandle, owner_id: String) -> Self;
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

#[derive(Default, Debug)]
pub struct Gotchi {
    archetype_handle: ArchetypeHandle,
    gp_harvested: usize,
}
archetype_deref!(Gotchi);
impl Possessable for Gotchi {
    type A = GotchiArchetype;
    const SIGN: char = 'G';
    fn new(archetype_handle: ArchetypeHandle, _owner_id: String) -> Self {
        Self {
            archetype_handle,
            ..Default::default()
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
        self.gp_harvested = item.get("gp_harvested")?.n.as_ref()?.parse().ok()?;
        Some(())
    }
    fn write_item(&self, item: &mut Item) {
        item.insert(
            "gp_harvested".to_string(),
            AttributeValue {
                n: Some(self.gp_harvested.to_string()),
                ..Default::default()
            },
        );
    }
}

#[derive(Default, Debug)]
pub struct Seed {
    archetype_handle: ArchetypeHandle,
    pedigree: Vec<SeedGrower>,
}
archetype_deref!(Seed);
#[derive(Default, Debug)]
pub struct SeedGrower {
    id: String,
    generations: usize,
}
impl Possessable for Seed {
    type A = SeedArchetype;
    const SIGN: char = 'G';
    fn new(archetype_handle: ArchetypeHandle, owner_id: String) -> Self {
        Self {
            archetype_handle,
            pedigree: vec![SeedGrower { id: owner_id, generations: 0 }],
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
            .filter_map(|v| {
                let m = v.m.as_ref()?;
                Some(SeedGrower {
                    id: m.get("id")?.s.as_ref()?.clone(),
                    generations: m.get("generations")?.n.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Some(())
    }
    fn write_item(&self, item: &mut Item) {
        item.insert(
            "pedigree".to_string(),
            AttributeValue {
                l: Some(
                    self.pedigree
                        .iter()
                        .map(|sg| AttributeValue {
                            m: Some(
                                [
                                    (
                                        "id".to_string(),
                                        AttributeValue {
                                            s: Some(sg.id.clone()),
                                            ..Default::default()
                                        },
                                    ),
                                    (
                                        "generations".to_string(),
                                        AttributeValue {
                                            n: Some(sg.generations.to_string()),
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
                        .collect(),
                ),
                ..Default::default()
            },
        );
    }
}

#[derive(Debug)]
pub struct Keepsake {
    archetype_handle: ArchetypeHandle,
}
archetype_deref!(Keepsake);
impl Possessable for Keepsake {
    type A = KeepsakeArchetype;
    const SIGN: char = 'K';
    fn new(archetype_handle: ArchetypeHandle, _owner: String) -> Self {
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

#[derive(Debug)]
pub struct Possessed<P: Possessable> {
    pub inner: P,
    pub archetype_handle: ArchetypeHandle,
    owner_history: Vec<String>,
    count: usize,
}

impl<P: Possessable> std::ops::Deref for Possessed<P> {
    type Target = Archetype;

    fn deref(&self) -> &Self::Target {
        self.archetype()
    }
}

impl<P: Possessable> Possessed<P> {
    pub fn new(archetype_handle: ArchetypeHandle, owner_id: String) -> Self {
        Self {
            inner: P::new(archetype_handle, owner_id.clone()),
            archetype_handle,
            count: 1,
            owner_history: vec![owner_id],
        }
    }

    fn archetype(&self) -> &Archetype {
        CONFIG
            .archetypes
            .get(self.archetype_handle)
            .expect("invalid archetype handle")
    }

    fn item(&self, slack_id: String) -> Item {
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
                s: Some({
                    let archetype_handle_string = self.archetype_handle.to_string();
                    let mut sk =
                        String::with_capacity(archetype_handle_string.len() + self.name.len() + 2);
                    sk.push(P::SIGN);
                    sk.push('#');
                    sk.push_str(&self.name);
                    sk.push('#');
                    sk.push_str(&archetype_handle_string);
                    sk
                }),
                ..Default::default()
            },
        );
        m.insert(
            "count".to_string(),
            AttributeValue {
                n: Some(self.count.to_string()),
                ..Default::default()
            },
        );
        m.insert(
            "owner_history".to_string(),
            AttributeValue {
                ss: Some(self.owner_history.clone()),
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
        let archetype_handle = sk_parts.next()?.to_owned().parse().ok()?;

        let mut inner = P::new(archetype_handle, item.get("pk")?.s.as_ref()?.to_string());
        inner.fill_from_item(item);

        Some(Self {
            inner,
            archetype_handle,
            owner_history: item.get("owner_history")?.ss.as_ref()?.clone(),
            count: item.get("count")?.n.as_ref()?.parse().ok()?,
        })
    }
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
