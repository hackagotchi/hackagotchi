use reqwest::Client;
use core::config;
use config::{AdvancementSet, Advancement, AdvancementSum};
use rocket::tokio;
use serde::{Serialize, Deserialize};

lazy_static::lazy_static! {
    pub static ref KEY: String = std::env::var("GOOGLE_CONFIG_KEY").unwrap();
}

/// A single Google Sheets Sheet.
#[derive(Serialize, Deserialize, Debug)]
struct Sheet {
    values: Vec<Vec<String>>
}
impl Sheet {
    /// turns a sheet into a list of advancements
    fn to_advancements<S: AdvancementSum>(self) -> Result<AdvancementSet<S>, &'static str> {
        // convert a single advancement
        fn to_advancement<S: AdvancementSum>(v: Vec<String>) -> Advancement<S> {
            let value = v.pop().ok_or("missing value");
            let kind = v.pop().ok_or("missing kind");

            Advancement {
                kind: serde_json::from_str(
                        &format!("{{ \"{}\": {} }}", kind, value)
                    )
                    .ok_or("invalid json"),
                achiever_title: v.pop().ok_or("missing achiever title"),
                description: v.pop().ok_or("missing description"),
                title: v.pop().ok_or("missing title"),
                xp: v.pop().ok_or("missing xp")
            }
        }

        AdvancementSet {
            base: to_advancement(
                self.values.remove(0).clone().ok_or("no advancements")?
            )?,
            rest: self
                .values
                .into_iter()
                .map(|x| to_advancement(x))
                .collect::<Result<Vec<_>, _>>()?
        }
    }
}

async fn yank_sheet(client: Client, id: String, name: String) -> Result<Sheet, String> {
    client
        .get(&dbg!(format!(
            concat!(
                "https://sheets.googleapis.com/v4/spreadsheets/{}/",
                "values/{}?key={}",
            ),
            id,
            name,
            *KEY
        )))
        .send()
        .await
        .map_err(|e| format!("couldn't grab from google: {}", e))?
        .json()
        .await
        .map_err(|e| format!("couldn't JSON: {}", e))
}

#[tokio::test]
// Note that this test relies on the Plants spreadsheet
// having a 'bractus' sheet, and may fail erroneously
// if that is not the case.
async fn yank_sheet_not_empty() {
    dotenv::dotenv().ok();
    
    let client = reqwest::Client::new();
    let s = yank_sheet(
        client,
        "129av9xlxkby72vkYOJrKHN1ybEM1EGY_YHGsFoSgGwo".to_string(),
        "bractus".to_string()
    )
    .await;

    assert!(s.is_ok(), "couldn't fetch sheet: {:?}", s)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    
    let client = reqwest::Client::new();

    Ok(())
}
