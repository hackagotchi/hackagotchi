use config::{
    Advancement, AdvancementSet, AdvancementSum, Archetype as PossessionArchetype,
    HacksteadAdvancementSet, PlantArchetype,
};
use hcor::config;
use reqwest::Client;
use rocket::tokio;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug)]
/// Every good configuration file management utility has a configuration file.
/// Short for "Config configuration" which is short for "configuration configuration"
struct CConfig {
    plants: PlantCConfig,
    hackstead_advancements_sheet_id: String,
    items_sheet_id: String,
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
    SheetError(String, SheetError),
    /// Invalid Name
    ArchetypeNameError(String),
    SerializeConfigError(serde_json::Error),
    WriteConfigError(std::io::Error),
}
impl fmt::Display for YankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use YankError::*;
        match self {
            RequestError(msg, e) => write!(f, "Request Error({}): {}", msg, e),
            SheetError(sheet_name, e) => write!(f, "Error parsing \"{}\" sheet: {}", sheet_name, e),
            ArchetypeNameError(e) => write!(f, "Archetype Name Error: {}", e),
            SerializeConfigError(e) => write!(f, "Error serializing the config: {}", e),
            WriteConfigError(e) => write!(f, "Error writing the config to a file: {}", e),
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
    /// unspecified JSON error
    JsonError(&'static str, serde_json::error::Error),
}
use SheetError::*;

impl fmt::Display for SheetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingCell(cell_desc, row) => {
                write!(f, "missing \"{}\" cell on row {}", cell_desc, row)
            }
            CellJsonError(cell_desc, json_err, full_json, row) => write!(
                f,
                concat!(
                    "Invalid json in \"{}\" cell on row {}\n",
                    "full json: \n```{}``` ",
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
            ),
            CellFloatParsingError(cell_desc, flt_err, row) => write!(
                f,
                "Invalid decimal number in \"{}\" cell on row {}: {}",
                cell_desc, row, flt_err,
            ),
            CellIntParsingError(cell_desc, int_err, row) => write!(
                f,
                "Invalid integer number in \"{}\" cell on row {}: {}",
                cell_desc, row, int_err,
            ),
            Missing(what) => write!(f, "missing {}", what),
            JsonError(msg, e) => write!(f, "json error: {}: {}", msg, e),
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
    values: Vec<Vec<String>>,
    name: String,
}
impl Sheet {
    /// turns a sheet into a list of advancements
    fn to_advancements<S: AdvancementSum>(
        self,
        row_offset: usize,
    ) -> Result<AdvancementSet<S>, SheetError> {
        // convert a single advancement
        fn to_advancement<S: AdvancementSum>(
            mut v: Vec<String>,
            row: usize,
        ) -> Result<Advancement<S>, SheetError> {
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
            .skip(row_offset)
            // adding one because google sheets starts at 1 not 0
            .map(|(i, x)| to_advancement(x, i + row_offset + 1))
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

    fn to_possession_archetypes(
        self,
        row_offset: usize,
    ) -> Result<Vec<PossessionArchetype>, SheetError> {
        // convert a single advancement
        fn row_to_possession_archetype(
            mut v: Vec<String>,
            row: usize,
        ) -> Result<PossessionArchetype, SheetError> {
            let value = v.pop().ok_or(MissingCell("Value", row))?;
            let kind = v.pop().ok_or(MissingCell("Kind", row))?;
            let kind_json = format!("{{ \"{}\": {} }}", kind, value);

            Ok(PossessionArchetype {
                kind: serde_json::from_str(&kind_json)
                    .map_err(|e| CellJsonError("value", e, kind_json, row))?,
                description: v.pop().ok_or(MissingCell("Description", row))?,
                name: v.pop().ok_or(MissingCell("Title", row))?,
            })
        }

        self.values
            .into_iter()
            .enumerate()
            .skip(row_offset)
            // adding one because google sheets starts at 1 not 0
            .map(|(i, x)| row_to_possession_archetype(x, i + row_offset + 1))
            .collect()
    }

    fn to_plant_archetype(self) -> Result<PlantArchetype, SheetError> {
        if self.values.is_empty() {
            return Err(Missing("entire sheet"));
        }
        let first_row = self.values.get(0).ok_or(Missing("first row"))?;

        let first_cell = first_row.first().ok_or(Missing("first cell"))?;
        let base_yield_duration = {
            let mut sections = first_cell.split(':');
            let _ = sections
                .next()
                .ok_or(Missing("first cell on the first row"))?;
            let base = sections.next().ok_or(Missing("colon in first cell"))?;
            match base.trim() {
                "NONE" => None,
                other => Some(other.parse().map_err(|e| {
                    CellFloatParsingError(
                        "number after colon in first cell for base yield duration",
                        e,
                        0,
                    )
                })?),
            }
        };

        Ok(PlantArchetype {
            name: self.name.clone(),
            base_yield_duration,
            advancements: {
                // because we yank out the first row (of titles) since
                // they're mostly for the humans and we need to find the base_yield_duration
                const PLANT_ARCHETYPE_ADVANCEMENT_ROW_OFFSET: usize = 1;
                self.to_advancements(PLANT_ARCHETYPE_ADVANCEMENT_ROW_OFFSET)?
            },
        })
    }
}

async fn yank_sheet(client: &Client, id: &str, name: String) -> Result<Sheet, YankError> {
    let v: serde_json::Value = client
        .get(&format!(
            concat!(
                "https://sheets.googleapis.com/v4/spreadsheets/{}/",
                "values/{}?key={}",
            ),
            id, name, *KEY
        ))
        .send()
        .await
        .map_err(|e| YankError::RequestError("couldn't grab from google", e))?
        .json()
        .await
        .map_err(|e| YankError::RequestError("Google's Sheet Json is faulty", e))?;

    Ok(Sheet {
        values: serde_json::from_value(
            v.get("values")
                .ok_or_else(|| {
                    YankError::SheetError(name.clone(), Missing("values field in sheet json"))
                })?
                .clone(),
        )
        .map_err(|e| {
            YankError::SheetError(
                name.clone(),
                JsonError("google sheet json has invalid values field", e),
            )
        })?,
        name,
    })
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
        "bractus".to_string(),
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
    use config::{PlantAdvancementKind, PlantAdvancementSet};

    let sheet = Sheet {
        name: "test plant".to_string(),
        values: vec![
            vec!["0", "Title", "Desc", "A_Title", "Art", "YieldSize", "1.1"],
            vec![
                "2",
                "Title2",
                "Desc2",
                "A_Title2",
                "Art2",
                "YieldSpeed",
                "2.2",
            ],
        ]
        .into_iter()
        .map(|v| v.into_iter().map(|x| x.to_string()).collect())
        .collect(),
    };

    let mut adv: PlantAdvancementSet = sheet.to_advancements(0).unwrap();

    // the one with 0 xp should become the base
    assert!(
        adv.base
            == Advancement {
                xp: 0,
                title: "Title".to_string(),
                description: "Desc".to_string(),
                achiever_title: "A_Title".to_string(),
                art: "Art".to_string(),
                kind: PlantAdvancementKind::YieldSizeMultiplier(1.1),
            }
    );

    // base should not be present in the "rest"
    assert!(adv.rest.len() == 1);

    // make sure the second advancement made its way to the "rest"
    let last = adv.rest.pop().expect("no last!");
    assert!(
        last == Advancement {
            xp: 2,
            title: "Title2".to_string(),
            description: "Desc2".to_string(),
            achiever_title: "A_Title2".to_string(),
            art: "Art2".to_string(),
            kind: PlantAdvancementKind::YieldSpeedMultiplier(2.2),
        }
    );
}

#[tokio::test]
async fn yank_config_full() {
    yank_config()
        .await
        .unwrap_or_else(|e| panic!("couldn't yank: {}", e))
}

pub async fn yank_config() -> Result<(), YankError> {
    use futures::stream::{self, StreamExt, TryStreamExt};

    let client = reqwest::Client::new();

    let (mut plants, hackstead_advancements, items): (
        Vec<PlantArchetype>,
        HacksteadAdvancementSet,
        Vec<PossessionArchetype>,
    ) = futures::try_join!(
        stream::iter(C_CONFIG.plants.include.clone())
            .map(|plant_name| async {
                yank_sheet(&client, &C_CONFIG.plants.sheet_id, plant_name.clone())
                    .await?
                    .to_plant_archetype()
                    .map_err(|e| YankError::SheetError(plant_name, e))
            })
            .buffer_unordered(50)
            .try_collect::<Vec<PlantArchetype>>(),
        async {
            yank_sheet(
                &client,
                &C_CONFIG.hackstead_advancements_sheet_id,
                "Hackstead Advancements".to_string(),
            )
            .await
            .and_then(|s| {
                s.to_advancements(1)
                    .map_err(|e| YankError::SheetError("Hackstead Advancements".to_string(), e))
            })
        },
        async {
            yank_sheet(&client, &C_CONFIG.items_sheet_id, "Items".to_string())
                .await
                .and_then(|s| {
                    s.to_possession_archetypes(1)
                        .map_err(|e| YankError::SheetError("Items".to_string(), e))
                })
        }
    )?;

    plants.sort_by_key(|p| {
        C_CONFIG
            .plants
            .include
            .iter()
            .position(|n| *n == p.name)
            .unwrap_or_else(|| panic!("couldn't find plant: {:?}", p.name))
    });

    let config = config::Config {
        special_users: vec![],
        profile_archetype: config::ProfileArchetype {
            advancements: hackstead_advancements,
        },
        plant_archetypes: plants,
        possession_archetypes: items,
    };

    config::check_archetype_name_matches(&config).map_err(|e| YankError::ArchetypeNameError(e))?;

    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("hcor/config/content.json"),
        &serde_json::to_string_pretty(&config).map_err(|e| YankError::SerializeConfigError(e))?,
    )
    .map_err(|e| YankError::WriteConfigError(e))
}
