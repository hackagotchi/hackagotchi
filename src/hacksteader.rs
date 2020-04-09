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
pub enum ArchetypeKind {
    Gotchi { power: usize },
    Seed,
    Item,
}
pub struct Archetype {
    name: String,
    kind: ArchetypeKind,
}
struct ArchetypeHandle(usize);

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
    pub gotchis: Vec<Gotchi>,
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

    pub async fn add_gotchi(
        db: &DynamoDbClient,
        user_id: String,
        gotchi: Gotchi,
    ) -> Result<(), RusotoError<PutItemError>> {
        db.put_item(rusoto_dynamodb::PutItemInput {
            item: gotchi.item(user_id),
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

        for item in items.iter() {
            (|| -> Option<()> {
                let sk = item.get("sk")?.s.as_ref()?;
                match sk.chars().next()? {
                    'P' => profile = Some(HacksteaderProfile::from_item(item)?),
                    'G' => gotchis.push(Gotchi::from_item(item)?),
                    _ => unreachable!(),
                }
                Some(())
            })();
        }
        Some(Hacksteader {
            profile: profile?,
            gotchis,
            user_id,
        })
    }
}

pub struct Gotchi {
    gp_harvested: usize,
}
pub struct Seed {
    pedigree: Vec<SeedGrower>,
}
pub struct SeedGrower {
    id: String,
    generations: usize,
}

pub enum PossessionKind {
    Gotchi(Gotchi),
    Seed(Seed),
    Item,
}
impl PossessionKind {
    fn sign(&self) -> char {
        match self {
            PossessionKind::Gotchi(_) => 'G',
            PossessionKind::Seed(_) => 'S',
            PossessionKind::Item => 'I',
        }
    }
    fn from_sign_item(sign: char, item: &Item) -> Option<Self> {
        Some(match sign {
            'G' => PossessionKind::Gotchi(Gotchi {
                gp_harvested: item.get("gp_harvested")?.n.as_ref()?.parse().ok()?,
            }),
            'S' => PossessionKind::Seed(Seed {
                pedigree: item
                    .get("pedigree")?
                    .l?
                    .filter_map(|v| {
                        Some(SeedGrower {
                            id: v.get("id")?.s?.clone(),
                            generations: v.get("generations")?.n.as_ref()?.parse().ok()?,
                        })
                    })
                    .collect(),
            }),
            'I' => PossessionKind::Item,
            _ => unreachable!(),
        })
    }
    fn write_item(&self, item: &mut Item) {
        match self {
            PossessionKind::Gotchi(Gotchi { gp_harvested }) => {
                item.insert(
                    "gp_harvested",
                    AttributeValue {
                        n: Some(gp_harvested.to_string()),
                        ..Default::default()
                    },
                );
            }
            PossessionKind::Seed(Seed { pedigree }) => {
                item.insert(
                    "pedigree",
                    AttributeValue {
                        l: Some(
                            pedigree
                                .map(|sg| AttributeValue {
                                    m: [
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
                                    ..Default::default()
                                })
                                .collect(),
                        ),
                        ..Default::default()
                    },
                );
            }
        }
    }
}
struct Possession {
    kind: PossessionKind,
    arch: ArchetypeHandle,
    owner_history: Vec<String>,
    count: usize,
}
impl Possession {
    pub fn new(kind: PossessionKind, arch: ArchetypeHandle) -> Self {
        Self {
            kind,
            arch,
            count: 1,
            owner_history: Vec::new(),
        }
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
                    let mut sk = String::with_capacity(self.name.len() + 2);
                    sk.push(self.kind.sign());
                    sk.push('#');
                    sk.push_str(&self.name);
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
        self.kind.write_item(&mut m);
        m
    }

    pub fn from_item(item: &Item) -> Option<Self> {
        let mut sk_parts = item.get("sk")?.s.as_ref()?.split("#");
        let sign_section_chars = sk_parts.next()?.chars();

        Some(Self {
            kind: PossessionKind::from_sign_item(sign_section_chars.next(), item),
            name: sk_parts.next()?.to_string(),
            owner_history: item.get("owner_history")?.ss?.clone(),
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
