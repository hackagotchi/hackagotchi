use humantime::{format_rfc3339, parse_rfc3339, format_duration};
use rusoto_dynamodb::AttributeValue;
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;

type Item = HashMap<String, AttributeValue>;

pub const TABLE_NAME: &'static str = "hacksteaders";

#[macro_export]
macro_rules! hacksteader_opening_blurb { ( $hackstead_cost:expr ) => { format!(
r#"
*Your Own Hackagotchi Homestead!*

:corn: Grow your own Farmables which make Hackagotchi more powerful!
:sparkling_heart: Earn passive income by collecting adorable Hackagotchi!
:money_with_wings: Buy, sell and trade Farmables and Hackagotchi at an open auction!

Hacksteading costs *{} GP*.
As a Hacksteader, you'll have a plot of land on which to grow your own Farmables, which can be fed to
Hackagotchi to make them more powerful. More powerful Hackagotchi generate more passive income!
You can also buy, sell, and trade Farmables and Hackagotchi on an open auction space.
"#,
$hackstead_cost
) } }


pub struct HacksteaderProfile {
    slack_id: String,
    /// Indicates when this Hacksteader first joined the elite community.
    joined: SystemTime,
}
impl fmt::Display for HacksteaderProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "*Hacksteader <@{}>*\n\n joined {} ago (roughly)",
            self.slack_id,
            format_duration(SystemTime::now().duration_since(self.joined).unwrap()),
        )
    }
}
impl HacksteaderProfile {
    pub fn new(slack_id: String) -> Self {
        Self {
            slack_id,
            joined: SystemTime::now(),
        }
    }

    /// Returns an empty profile Item for the given slack ID.
    /// Useful for searching for a given slack user's Hacksteader profile
    pub fn empty_item(slack_id: String) -> Item {
        let mut m = Item::new();
        m.insert(
            "slack_id".to_string(),
            AttributeValue {
                s: Some(slack_id),
                ..Default::default()
            },
        );
        m.insert(
            "sk".to_string(),
            AttributeValue {
                s: Some("profile".to_string()),
                ..Default::default()
            },
        );
        m
    }

    pub fn item(&self) -> Item {
        let mut m = Self::empty_item(self.slack_id.clone());
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
            slack_id: item.get("slack_id")?.s.as_ref()?.to_string(),
            joined: item
                .get("joined")
                .and_then(|a| a.s.as_ref())
                .and_then(|s| parse_rfc3339(s).ok())
                .unwrap_or_else(|| SystemTime::now()),
        })
    }
}
