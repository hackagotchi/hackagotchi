use humantime::{format_rfc3339, parse_rfc3339};
use rusoto_core::RusotoError;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient, PutItemError};
use std::collections::HashMap;
use std::time::SystemTime;

type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hackagotchi";

#[derive(serde::Deserialize)]
pub struct Config {
    special_users: Vec<String>,
    archetypes: HashMap<String, Archetype>,
}

#[derive(serde::Deserialize)]
pub struct GotchiArchetype {
    pub power: usize,
}
#[derive(serde::Deserialize)]
pub struct SeedArchetype;
#[derive(serde::Deserialize)]
pub struct KeepsakeArchetype;

#[derive(serde::Deserialize)]
pub enum ArchetypeKind {
    Gotchi(GotchiArchetype),
    Seed(SeedArchetype),
    Keepsake(KeepsakeArchetype),
}
#[derive(serde::Deserialize)]
pub struct Archetype {
    pub display_name: String,
    kind: ArchetypeKind,
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

    pub async fn possess_new<P: Possessable>(
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

pub trait Possessable: Sized {
    /// The archetype that corresponds to this Possessable
    type A;

    /// A char used in the ids that this Possessable serializes into in the database.
    const SIGN: char;

    fn new(archetype_handle: &str) -> Self;
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A>;
    fn from_item(item: &Item) -> Option<Self>;
    fn write_item(&self, item: &mut Item);
}

#[derive(Default)]
pub struct Gotchi {
    gp_harvested: usize,
}
impl Possessable for Gotchi {
    type A = GotchiArchetype;
    const SIGN: char = 'G';
    fn new(_archetype_handle: &str) -> Self {
        Default::default()
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Gotchi(ga) => Some(ga),
            _ => None,
        }
    }
    fn from_item(item: &Item) -> Option<Self> {
        Some(Gotchi {
            gp_harvested: item.get("gp_harvested")?.n.as_ref()?.parse().ok()?,
        })
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

#[derive(Default)]
pub struct Seed {
    pedigree: Vec<SeedGrower>,
}
pub struct SeedGrower {
    id: String,
    generations: usize,
}
impl Possessable for Seed {
    type A = SeedArchetype;
    const SIGN: char = 'G';
    fn new(_archetype_handle: &str) -> Self {
        Default::default()
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Seed(sa) => Some(sa),
            _ => None,
        }
    }
    fn from_item(item: &Item) -> Option<Self> {
        Some(Seed {
            pedigree: item
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
                .collect(),
        })
    }
    fn write_item(&self, item: &mut Item) {
        item.insert(
            "pedigree".to_string(),
            AttributeValue {
                l: Some(
                    self.pedigree
                        .iter()
                        .map(|sg| AttributeValue {
                            m: Some([
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
                            .collect()),
                            ..Default::default()
                        })
                        .collect(),
                ),
                ..Default::default()
            },
        );
    }
}

pub struct Keepsake;
impl Possessable for Keepsake {
    type A = KeepsakeArchetype;
    const SIGN: char = 'K';
    fn new(_archetype_handle: &str) -> Self {
        Keepsake
    }
    fn archetype_kind(a: &ArchetypeKind) -> Option<&Self::A> {
        match a {
            ArchetypeKind::Keepsake(ka) => Some(ka),
            _ => None,
        }
    }
    fn from_item(item: &Item) -> Option<Self> {
        Some(Keepsake)
    }
    fn write_item(&self, item: &mut Item) {}
}

pub struct Possessed<P: Possessable> {
    inner: P,
    pub archetype_handle: String,
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
    pub fn new(archetype_handle: String) -> Self {
        Self {
            inner: P::new(&archetype_handle),
            archetype_handle,
            count: 1,
            owner_history: Vec::new(),
        }
    }

    fn archetype(&self) -> &Archetype {
        CONFIG.archetypes.get(&self.archetype_handle).expect("invalid archetype handle")
    }

    pub fn inner_arch(&self) -> &P::A {
        P::archetype_kind(&self.archetype().kind)
            .unwrap_or_else(|| panic!("had '{}' archetype handle, got wrong ArchetypeKind"))
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
                    let mut sk = String::with_capacity(self.archetype_handle.len() + 2);
                    sk.push(P::SIGN);
                    sk.push('#');
                    sk.push_str(&self.archetype_handle);
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

        Some(Self {
            inner: P::from_item(item)?,
            archetype_handle: sk_parts.next()?.to_string(),
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
