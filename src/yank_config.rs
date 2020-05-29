#![feature(vec_remove_item)]
use reqwest::Client;
use core::config;
use config::{AdvancementSet, PlantArchetype, Advancement, AdvancementSum};
use rocket::tokio;
use serde::{Serialize, Deserialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug)]
/// Every good configuration file management utility has a configuration file.
/// Short for "Config configuration" which is short for "configuration configuration"
struct CConfig {
    plants: PlantCConfig,
}
#[derive(Serialize, Deserialize, Debug)]
struct PlantCConfig {
    sheet_id: String,
    include: Vec<String>,
}

#[derive(Debug)]
pub enum YankError {
    /// Contains: Message, Error
    RequestError(&'static str, reqwest::Error),
    /// Contains: Sheet Name, Error
    SheetError(String, SheetError)
}
impl fmt::Display for YankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use YankError::*;
        match self {
            RequestError(msg, e) => write!(f, "Request Error({}): {}", msg, e),
            SheetError(sheet_name, e) => {
                write!(f, "Error parsing \"{}\" sheet: {}", sheet_name, e)
            }
        }
    }
}

#[derive(Debug)]
pub enum SheetError {
    /// Contains: Column Header, row
    MissingCell(&'static str, usize),
    /// Contains: Column Header, Json Error, Json String, row
    CellJsonError(&'static str, serde_json::error::Error, String, usize),
    /// Contains: Column Header, Float Parsing Error, row
    CellFloatParsingError(&'static str, std::num::ParseFloatError, usize),
    /// Contains: Column Header, Integer Parsing Error, row
    CellIntParsingError(&'static str, std::num::ParseIntError, usize),
    /// Contains a description of that which is missing.
    Missing(&'static str),
}
use SheetError::*;

impl fmt::Display for SheetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingCell(cell_desc, row) => {
                write!(f, "missing \"{}\" cell on row {}", cell_desc, row)
            }
            CellJsonError(cell_desc, json_err, full_json, row) => {
                write!(
                    f,
                    concat!(
                        "Invalid json in \"{}\" cell on row {}\n",
                        "full json: \n{}\n\n",
                        "error: {}\n",
                    ),
                    cell_desc,
                    row,
                    full_json
                        .split('\n')
                        .enumerate()
                        .map(|(i, s)| format!("{:>3} | {}", i + 1, s))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    json_err,
                )
            }
            CellFloatParsingError(cell_desc, flt_err, row) => {
                write!(
                    f,
                    "Invalid decimal number in \"{}\" cell on row {}: {}",
                    cell_desc,
                    row,
                    flt_err,
                )
            }
            CellIntParsingError(cell_desc, int_err, row) => {
                write!(
                    f,
                    "Invalid integer number in \"{}\" cell on row {}: {}",
                    cell_desc,
                    row,
                    int_err,
                )
            }
            Missing(what) => write!(f, "missing {}", what),
        }
    }
}

lazy_static::lazy_static! {
    static ref KEY: String = std::env::var("GOOGLE_CONFIG_KEY").unwrap();

    // Every good configuration file management utility has a configuration file.
    static ref C_CONFIG: CConfig = {
        const PATH: &'static str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/config/config_config.json"
        );

        serde_json::from_str(
                &std::fs::read_to_string(PATH)
                    .unwrap_or_else(|e| panic!("opening {}: {}", PATH, e))
            )
            .unwrap_or_else(|e| panic!("parsing {}: {}", PATH, e))
    };
}

#[derive(Serialize, Deserialize, Debug, Default)]
/// A single Google Sheets Sheet.
struct Sheet {
    values: Vec<Vec<String>>
}
impl Sheet {
    /// turns a sheet into a list of advancements
    fn to_advancements<S: AdvancementSum>(self, row_offset: usize) -> Result<AdvancementSet<S>, SheetError> {
        // convert a single advancement
        fn to_advancement<S: AdvancementSum>(mut v: Vec<String>, row: usize) -> Result<Advancement<S>, SheetError> {
            let value = v.pop().ok_or(MissingCell("Value", row))?;
            let kind = v.pop().ok_or(MissingCell("Kind", row))?;
            let kind_json = format!("{{ \"{}\": {} }}", kind, value);

            Ok(Advancement {
                kind: serde_json::from_str(&kind_json)
                    .map_err(|e| CellJsonError("value", e, kind_json, row))?,
                art: v.pop().ok_or(MissingCell("Art", row))?,
                achiever_title: v.pop().ok_or(MissingCell("Achiever Title", row))?,
                description: v.pop().ok_or(MissingCell("Description", row))?,
                title: v.pop().ok_or(MissingCell("Title", row))?,
                xp: v
                    .pop()
                    .ok_or(MissingCell("Xp", row))?
                    .parse()
                    .map_err(|e| CellIntParsingError("xp", e, row))?,
            })
        }

        let mut advancements = self
            .values
            .into_iter()
            .enumerate()
            .map(|(i, x)| to_advancement(x, i + row_offset))
            .collect::<Result<Vec<_>, _>>()?;
        let base_index = advancements
            .iter()
            .position(|x| x.xp == 0)
            .ok_or(Missing("starting advancement (one with 0xp)"))?;

        Ok(AdvancementSet {
            base: advancements.remove(base_index),
            rest: advancements,
        })
    }

    fn to_plant_archetype(mut self, name: &'static str) -> Result<PlantArchetype, SheetError> {
        if self.values.is_empty() {
            return Err(Missing("entire sheet"));
        }
        let first_row = self.values.remove(0);

        let first_cell = first_row.first().ok_or(Missing("first cell"))?;
        let base_yield_duration = {
            let mut sections = first_cell.split(':');
            let _ = sections.next().ok_or(Missing("first cell on the first row"))?;
            let base = sections.next().ok_or(Missing("colon in first cell"))?;
            base.trim().parse().map_err(|e| CellFloatParsingError("number after colon in first cell for base yield duration", e, 0))?
        };

        Ok(PlantArchetype {
            name: name.to_string(),
            base_yield_duration,
            advancements: {
                // one one is necessary because we yank out the
                // first row (of titles) since they're mostly for the humans
                // and we need to find the base_yield_duration
                // another one is necessary because google sheets starts
                // at one.
                const PLANT_ARCHETYPE_ADVANCEMENT_ROW_OFFSET: usize = 1 + 1;
                self.to_advancements(PLANT_ARCHETYPE_ADVANCEMENT_ROW_OFFSET)?
            }
        })
    }
}

async fn yank_sheet(client: &Client, id: &str, name: &str) -> Result<Sheet, YankError> {
    client
        .get(&format!(
            concat!(
                "https://sheets.googleapis.com/v4/spreadsheets/{}/",
                "values/{}?key={}",
            ),
            id,
            name,
            *KEY
        ))
        .send()
        .await
        .map_err(|e| YankError::RequestError("couldn't grab from google", e))?
        .json()
        .await
        .map_err(|e| YankError::RequestError("Google's Sheet Json is faulty", e))
}

#[tokio::test]
// Note that this test relies on the Plants spreadsheet
// having a 'bractus' sheet, and may fail erroneously
// if that is not the case.
async fn yank_sheet_not_empty() {
    dotenv::dotenv().ok();
    
    let client = reqwest::Client::new();
    let s = yank_sheet(
        &client,
        "129av9xlxkby72vkYOJrKHN1ybEM1EGY_YHGsFoSgGwo",
        "bractus"
    )
    .await;

    assert!(s.is_ok(), "couldn't fetch sheet: {:?}", s)
}

#[test]
fn load_config() {
    dotenv::dotenv().ok();

    println!("{:#?}", *C_CONFIG);
}

#[test]
fn sheet_to_advancement() {
    use config::{PlantAdvancementSet, PlantAdvancementKind};

    let sheet = Sheet {
        values: vec![
            vec!["0", "Title", "Desc", "A_Title", "Art", "YieldSize", "1.1"],
            vec!["2", "Title2", "Desc2", "A_Title2", "Art2", "YieldSpeed", "2.2"],
        ]
        .into_iter()
        .map(|v| v.into_iter().map(|x| x.to_string()).collect())
        .collect()
    };

    let mut adv: PlantAdvancementSet = sheet.to_advancements().unwrap();

    // the one with 0 xp should become the base
    assert!(adv.base == Advancement {
        xp: 0,
        title: "Title".to_string(),
        description: "Desc".to_string(),
        achiever_title: "A_Title".to_string(),
        art: "Art".to_string(),
        kind: PlantAdvancementKind::YieldSize(1.1),
    });

    // base should not be present in the "rest"
    assert!(adv.rest.len() == 1);

    // make sure the second advancement made its way to the "rest"
    let last = adv.rest.pop().expect("no last!");
    assert!(last == Advancement {
        xp: 2,
        title: "Title2".to_string(),
        description: "Desc2".to_string(),
        achiever_title: "A_Title2".to_string(),
        art: "Art2".to_string(),
        kind: PlantAdvancementKind::YieldSpeed(2.2),
    });
}

#[tokio::main]
async fn yank_config() -> Result<(), YankError> {
    use futures::stream::{self, TryStreamExt, StreamExt};
    use futures::future::TryFutureExt;

    dotenv::dotenv().ok();
    
    let client = reqwest::Client::new();

    let plant_archetypes: Vec<PlantArchetype> = stream::iter(&C_CONFIG.plants.include)
        .map(|plant_name| {
            yank_sheet(
                &client,
                &C_CONFIG.plants.sheet_id,
                plant_name
            )
            .and_then(move |s| async move {
                s
                    .to_plant_archetype(plant_name)
                    .map_err(|e| YankError::SheetError(plant_name.clone(), e))
            })
        })
        .buffer_unordered(50)
        .try_collect::<Vec<_>>()
        .await
        .unwrap_or_else(|e| panic!("couldn't yank sheet: {}", e));

    println!("{:#?}", plant_archetypes);

    Ok(())
}
