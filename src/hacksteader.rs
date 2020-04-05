use humantime::{format_rfc3339, parse_rfc3339};
use rusoto_dynamodb::{AttributeValue, DynamoDbClient, DynamoDb};
use rusoto_core::RusotoError;
use std::collections::HashMap;
use std::time::SystemTime;

type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hackagotchi";


pub struct Hacksteader {
    pub user_id: String,
    pub profile: HacksteaderProfile,
    pub gotchis: Vec<Gotchi>,
}
impl Hacksteader {
    pub async fn new_in_db(db: &DynamoDbClient, user_id: String) -> Result<(), RusotoError<rusoto_dynamodb::PutItemError>> {
        // just give them a profile for now
        db.put_item(rusoto_dynamodb::PutItemInput {
            item: HacksteaderProfile::new().item(user_id),
            table_name: TABLE_NAME.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
    }
    pub async fn from_db(db: &DynamoDbClient, user_id: String) -> Option<Self> {
        let query = db.query(rusoto_dynamodb::QueryInput {
            table_name: TABLE_NAME.to_string(),
            key_condition_expression: Some("id = :db_id".to_string()),
            expression_attribute_values: Some({
                let mut m = HashMap::new();
                m.insert(":db_id".to_string(), AttributeValue {
                    s: Some(user_id.clone()),
                    ..Default::default()
                });
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
                    _ => unreachable!()
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
    pub name: String,
    pub id: String,
}
impl Gotchi {
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
                    let mut sk = String::with_capacity(self.id.len() + self.name.len() + 3);
                    sk.push('G');
                    sk.push('#');
                    sk.push_str(&self.name);
                    sk.push('#');
                    sk.push_str(&self.id);
                    sk
                }),
                ..Default::default()
            },
        );
        m
    }

    pub fn from_item(item: &Item) -> Option<Self> {
        let mut sk_parts = item.get("sk")?.s.as_ref()?.split("#");
        
        //you did give us a gotchi ID, right?
        assert_eq!("G", sk_parts.next()?);

        Some(Self {
            name: sk_parts.next()?.to_string(),
            id: sk_parts.next()?.to_string()
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
