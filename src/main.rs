#![warn(missing_docs)]
#![feature(decl_macro)]
#![feature(proc_macro_hygiene)]
#![feature(try_trait)]
#![recursion_limit = "512"]
use config::CONFIG;
use crossbeam_channel::Sender;
use hcor::config;
use hcor::frontend::emojify;
use hcor::possess;
use hcor::{Category, Key};
use log::*;
use possess::{Possessed, Possession};
use regex::Regex;
use rocket::request::LenientForm;
use rocket::tokio;
use rocket::{post, routes, FromForm, State};
use rocket_contrib::json::Json;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::TryInto;

pub mod banker;
pub mod event;
pub mod hacksteader;
pub mod market;
mod yank_config;

use hacksteader::Hacksteader;

pub fn dyn_db() -> DynamoDbClient {
    DynamoDbClient::new(if *LOCAL_DB {
        rusoto_core::Region::Custom {
            name: "local".to_string(),
            endpoint: "http://localhost:8000".to_string(),
        }
    } else {
        rusoto_core::Region::UsEast1
    })
}

const FARM_CYCLE_SECS: u64 = 5;
const FARM_CYCLE_MILLIS: u64 = FARM_CYCLE_SECS * 1000;
const FARM_CYCLES_PER_MIN: u64 = 60 / FARM_CYCLE_SECS;

lazy_static::lazy_static! {
    pub static ref TOKEN: String = std::env::var("TOKEN").unwrap();
    pub static ref ID: String = std::env::var("ID").unwrap();
    pub static ref APP_ID: String = std::env::var("APP_ID").unwrap();
    pub static ref URL: String = std::env::var("URL").unwrap();
    pub static ref HACKSTEAD_PRICE: u64 = std::env::var("HACKSTEAD_PRICE").unwrap().parse().unwrap();
    pub static ref LOCAL_DB: bool = std::env::var("LOCAL_DB").is_ok();
}

pub fn mrkdwn<S: std::string::ToString>(txt: S) -> Value {
    json!({
        "type": "mrkdwn",
        "text": txt.to_string(),
    })
}
pub fn plain_text<S: std::string::ToString>(txt: S) -> Value {
    json!({
        "type": "plain_text",
        "text": txt.to_string(),
    })
}
pub fn comment<S: ToString>(txt: S) -> Value {
    json!({
        "type": "context",
        "elements": [
            mrkdwn(txt)
        ]
    })
}
pub fn filify<S: ToString>(txt: S) -> String {
    txt.to_string().to_lowercase().replace(" ", "_")
}

pub async fn dm_blocks(
    user_id: String,
    notif_msg: String,
    blocks: Vec<Value>,
) -> Result<(), String> {
    let o = json!({
        "channel": user_id,
        "token": *TOKEN,
        "blocks": blocks,
        "text": notif_msg
    });

    debug!("{}", serde_json::to_string_pretty(&o).unwrap());

    // TODO: use response
    let client = reqwest::Client::new();
    client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(&*TOKEN)
        .json(&o)
        .send()
        .await
        .map_err(|e| format!("couldn't dm {}: {}", user_id, e))?;

    Ok(())
}

async fn gift_dm(
    giver: &str,
    new_owner: &str,
    possession: &Possession,
    count: usize,
) -> Result<(), String> {
    dm_blocks(new_owner.to_string(), "You've received a gift!".to_string(), {
        // TODO: with_capacity optimization
        let mut blocks = vec![
            json!({
                "type": "section",
                "text": mrkdwn(format!(
                    "<@{}> has been so kind as to gift you {} {} _{}_!",
                    giver,
                    match count {
                        1 => "a".to_string(),
                        other => format!("*{}*", other),
                    },
                    emojify(&possession.name),
                    possession.nickname()
                ))
            }),
            json!({ "type": "divider" }),
        ];
        let page = PossessionPage {
            interactivity: Interactivity::Read,
            credentials: Credentials::Owner,
            possession: possession.clone(),
        };
        blocks.append(&mut page.blocks());
        blocks.push(json!({ "type": "divider" }));
        blocks.push(comment(format!(
            "Manage all of your possessions like this one at your <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>",
            *APP_ID,
        )));
        blocks
    })
    .await
}

/// `push` should be true if this modal is being put on top of an existing one.
fn gotchi_block(
    gotchi: Possessed<possess::Gotchi>,
    interactivity: Interactivity,
    credentials: Credentials,
    push: bool,
) -> Value {
    json!({
        "type": "section",
        "text": mrkdwn(format!(
            "_{} ({}, {})_",
            emojify(&gotchi.name),
            gotchi.name,
            match gotchi.inner.hatch_table {
                None => format!("{} happiness", gotchi.inner.base_happiness),
                Some(_) => "ready to hatch!".to_string(),
            }
        )),
        "accessory": {
            "type": "button",
            "style": "primary",
            "text": plain_text(&gotchi.inner.nickname),
            "value": serde_json::to_string(&PossessionPage {
                possession: gotchi.into_possession(),
                interactivity,
                credentials,
            }).unwrap(),
            "action_id": match push {
                true => "push_possession_page",
                _ => "possession_page",
            }
        }
    })
}

fn inventory_occurences(inventory: Vec<Possession>) -> HashMap<String, Vec<Possession>> {
    let mut o = HashMap::new();

    for possession in inventory.into_iter() {
        o.entry(possession.name.clone())
            .or_insert(vec![])
            .push(possession)
    }

    o
}

fn inventory_section(
    inv_occurrences: HashMap<String, Vec<Possession>>,
    interactivity: Interactivity,
    credentials: Credentials,
    push: bool,
    user_id: String,
) -> Vec<Value> {
    let mut blocks = vec![];

    blocks.push(json!({
        "type": "section",
        "text": mrkdwn("*Inventory*"),
    }));

    let mut inv_entries = inv_occurrences.into_iter().collect::<Vec<_>>();
    inv_entries.sort_unstable_by_key(|(_, p)| p.last().unwrap().archetype_handle);
    for (name, possessions) in inv_entries.into_iter() {
        // this is safe because you have to have at least one
        // for it to end up here
        let last = possessions.last().unwrap().clone();

        if possessions.len() == 1 {
            blocks.push(json!({
                "type": "section",
                "text": mrkdwn(format!(
                    "{} _{}_",
                    emojify(&name),
                    name
                )),
                "accessory": {
                    "type": "button",
                    "style": "primary",
                    "text": plain_text(&name),
                    "value": serde_json::to_string(&PossessionPage {
                        possession: last,
                        interactivity,
                        credentials,
                    }).unwrap(),
                    "action_id": match push {
                        false => "possession_page",
                        true => "push_possession_page"
                    },
                }
            }));
        } else {
            blocks.push(json!({
                "type": "section",
                "text": mrkdwn(format!(
                    "*{}* {} _{}_",
                    possessions.len(),
                    emojify(&name),
                    name
                )),
                "accessory": {
                    "type": "button",
                    "style": "primary",
                    "text": plain_text(&name),
                    "value": serde_json::to_string(&PossessionOverviewPage {
                        source: PossessionOverviewSource::Hacksteader(user_id.clone()),
                        page: 0,
                        item_name: name,
                        interactivity,
                        credentials,
                    }).unwrap(),
                    "action_id": match push {
                        false => "possession_overview_page",
                        true => "push_possession_overview_page"
                    },
                }
            }));
        }
    }

    blocks
}

fn gotchi_section(
    gotchis: Vec<Possessed<possess::Gotchi>>,
    interactivity: Interactivity,
    credentials: Credentials,
    push: bool,
) -> Vec<Value> {
    let mut blocks = vec![];

    blocks.push(json!({
        "type": "section",
        "text": mrkdwn(match gotchis.len() {
            1 => "*Your Hackagotchi*".into(),
            _ => format!("*Your {} Hackagotchi*", gotchis.len())
        }),
    }));

    let total_happiness = gotchis.iter().map(|g| g.inner.base_happiness).sum::<u64>();

    for g in gotchis.into_iter().take(20) {
        blocks.push(gotchi_block(g, interactivity, credentials, push));
    }

    blocks.push(json!({
        "type": "section",
        "text": mrkdwn(format!("Total happiness: *{}*", total_happiness))
    }));
    blocks.push(comment(
        "The total happiness of all your gotchi is equivalent to the \
         amount of GP you'll get at the next Harvest.",
    ));

    blocks
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub enum PossessionOverviewSource {
    Hacksteader(String),
    Market(Category),
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct PossessionOverviewPage {
    source: PossessionOverviewSource,
    page: usize,
    item_name: String,
    interactivity: Interactivity,
    credentials: Credentials,
}
impl PossessionOverviewPage {
    const PAGE_SIZE: usize = 20;

    fn title(&self) -> String {
        let mut title = self.item_name.clone();
        if title.len() > 24 {
            title.truncate(24 - 3);
            title.push_str("...");
        }
        title
    }

    async fn modal(self, trigger_id: String, method: &'static str) -> Result<Modal, String> {
        Ok(Modal {
            callback_id: self.callback_id(),
            blocks: self.blocks().await?,
            submit: None,
            title: self.title(),
            method: method.to_string(),
            trigger_id,
            private_metadata: String::new(),
        })
    }

    /*fn modal_update(self, trigger_id: String, page_json: String, view_id: String) -> ModalUpdate {
        ModalUpdate {
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            submit: None,
            title: self.name,
            private_metadata: page_json,
            trigger_id,
            view_id,
            hash: None,
        }
    }*/

    fn callback_id(&self) -> String {
        "possession_overview_page_".to_string() + self.interactivity.id()
    }

    async fn blocks(&self) -> Result<Vec<Value>, String> {
        let Self {
            source,
            item_name,
            page,
            credentials,
            interactivity,
        } = self;

        let inventory: Vec<_> = match source {
            PossessionOverviewSource::Hacksteader(hacksteader) => {
                let hs = Hacksteader::from_db(&dyn_db(), hacksteader.clone()).await?;
                let mut inv: Vec<_> = hs
                    .inventory
                    .into_iter()
                    .filter(|i| i.name == *item_name)
                    .collect();
                inv.sort_unstable_by(|a, b| match (a.sale.as_ref(), b.sale.as_ref()) {
                    (Some(a), Some(b)) => a.price.cmp(&b.price),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                });
                inv
            }
            PossessionOverviewSource::Market(cat) => market::market_search(&dyn_db(), *cat)
                .await
                .map_err(|e| error!("couldn't search market: {}", e))
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, i)| i.name == *item_name)
                .map(|(sale, mut possession)| {
                    possession.sale.replace(sale);
                    possession
                })
                .collect(),
        };
        // caching this so I can use it later after .into_iter() is called
        let inventory_len = inventory.len();
        let first_item = inventory.first().cloned();

        let mut blocks = inventory
            .into_iter()
            .skip(page * Self::PAGE_SIZE)
            .take(Self::PAGE_SIZE)
            .map(|possession| {
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "{} _{}_{}",
                        emojify(&item_name),
                        item_name,
                        if let Some(hcor::market::Sale { price, .. }) = possession.sale {
                            match source {
                                PossessionOverviewSource::Hacksteader(..) => {
                                    format!(" (selling at *{}gp*)", price)
                                },
                                PossessionOverviewSource::Market(..) => {
                                    format!(
                                        " (sold by *<@{}>*)",
                                        possession.steader,
                                    )
                                }
                            }
                        } else {
                            "".to_string()
                        }
                    )),
                    "accessory": {
                        "type": "button",
                        "style": "primary",
                        "text": plain_text(match (source, possession.sale.as_ref()) {
                            (PossessionOverviewSource::Market(..), Some(s)) => {
                                format!("{}gp", s.price)
                            }
                            _ => item_name.clone(),
                        }),
                        "value": serde_json::to_string(&(
                            possession,
                            interactivity,
                            credentials
                        )).unwrap(),
                        "action_id": "push_possession_page",
                    }
                })
            })
            .collect::<Vec<_>>();

        if let Some(p) = first_item.filter(|p| p.kind.is_keepsake()) {
            blocks.push(comment(format!(
                "hmm, maybe \"*/hgive <@U01581HFAGZ> {} {}*\" is in your future?",
                inventory_len,
                emojify(&p.name)
            )));
        }

        let needs_back_page = *page != 0;
        let needs_next_page = inventory_len > Self::PAGE_SIZE * (page + 1);
        if needs_back_page || needs_next_page {
            blocks.push(json!({
                "type": "actions",
                "elements": ({
                    let mut buttons = vec![];
                    let mut current_page = self.clone();

                    if needs_back_page {
                        let mut back_page = &mut current_page;
                        back_page.page = *page - 1;
                        buttons.push(json!({
                            "type": "button",
                            "text": plain_text("Back Page"),
                            "style": "primary",
                            "value": serde_json::to_string(back_page).unwrap(),
                            "action_id": "possession_overview_page"
                        }));
                    }
                    if needs_next_page {
                        let mut next_page = &mut current_page;
                        next_page.page = *page + 1;
                        buttons.push(json!({
                            "type": "button",
                            "text": plain_text("Next Page"),
                            "style": "primary",
                            "value": serde_json::to_string(next_page).unwrap(),
                            "action_id": "possession_overview_page"
                        }));
                    }

                    buttons
                })
            }));
        }

        Ok(blocks)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PossessionPage {
    possession: Possession,
    interactivity: Interactivity,
    credentials: Credentials,
}

impl PossessionPage {
    fn title(&self) -> String {
        let mut title = self.possession.nickname().to_string();
        if title.len() > 24 {
            title.truncate(24 - 3);
            title.push_str("...");
        }
        title
    }

    fn modal(self, trigger_id: String, method: &'static str) -> Modal {
        Modal {
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            submit: self.submit(),
            title: self.title(),
            method: method.to_string(),
            trigger_id: trigger_id,
            private_metadata: serde_json::to_string(&self.possession.key()).unwrap(),
        }
    }

    fn modal_update(self, trigger_id: String, view_id: String) -> ModalUpdate {
        ModalUpdate {
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            submit: self.submit(),
            title: self.title(),
            private_metadata: serde_json::to_string(&self.possession.key()).unwrap(),
            trigger_id,
            view_id,
            hash: None,
        }
    }

    fn submit(&self) -> Option<String> {
        if let Some(sale) = self
            .possession
            .sale
            .as_ref()
            .filter(|_| self.interactivity.market(self.credentials))
        {
            match self.credentials {
                Credentials::Owner => return Some("Take off Market".to_string()),
                Credentials::Hacksteader => return Some(format!("Buy for {} gp", sale.price)),
                _ => {}
            }
        }
        None
    }

    fn callback_id(&self) -> String {
        if self
            .possession
            .sale
            .as_ref()
            .filter(|_| self.interactivity.market(self.credentials))
            .is_some()
        {
            match self.credentials {
                Credentials::Owner => return "sale_removal".to_string(),
                Credentials::Hacksteader => return "sale_complete".to_string(),
                _ => {}
            }
        }
        "possession_page_".to_string() + self.interactivity.id()
    }

    fn blocks(&self) -> Vec<Value> {
        // TODO: with_capacity optimization
        let mut blocks: Vec<Value> = Vec::new();
        let Self {
            possession,
            interactivity,
            credentials,
        } = self;

        let actions = |prefix: &str, buttons: &[(&str, Option<Value>)]| -> Value {
            match interactivity {
                Interactivity::Write => json!({
                    "type": "actions",
                    "elements": buttons.iter().map(|(action, value)| {
                        let mut o = json!({
                            "type": "button",
                            "text": plain_text(action),
                            "action_id": format!("{}_{}", prefix, action.to_lowercase()),
                        });
                        if let Some(v) = value {
                            o.as_object_mut().unwrap().insert("value".to_string(), v.clone());
                        }
                        o
                    }).collect::<Vec<_>>()
                }),
                _ => comment("This page is read only."),
            }
        };

        if let Some(g) = possession.kind.gotchi() {
            blocks.push(actions("gotchi", &{
                let mut a = vec![("Nickname", Some(json!(possession.nickname())))];
                if g.hatch_table.is_some() {
                    a.push((
                        "Hatch",
                        Some(json!(serde_json::to_string(&possession.id).unwrap())),
                    ));
                }
                a
            }));
        }

        let mut text_fields = vec![
            ("description", possession.description.clone()),
            ("kind", possession.name.clone()),
            (
                "owner log",
                possession
                    .ownership_log
                    .iter()
                    .map(|o| format!("[{}]<@{}>", o.acquisition, o.id))
                    .collect::<Vec<_>>()
                    .join(" -> ")
                    .to_string(),
            ),
        ];

        if let Some(g) = possession.kind.gotchi() {
            text_fields.push(("base happiness", g.base_happiness.to_string()));
        }

        let text = text_fields
            .iter()
            .map(|(l, r)| format!("*{}:* _{}_", l, r))
            .collect::<Vec<_>>()
            .join("\n");

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(text),
            "accessory": {
                "type": "image",
                "image_url": format!(
                    "http://{}/gotchi/img/{}/{}.png",
                    *URL,
                    format!("{:?}", possession.kind.category()).to_lowercase(),
                    filify(&possession.name)
                ),
                "alt_text": "hackagotchi img",
            }
        }));

        blocks.push(actions("possession", &[("Give", None), ("Sell", None)]));

        if let Some(g) = possession.kind.gotchi() {
            blocks.push(comment(format!(
                "*Lifetime GP harvested: {}*",
                g.harvest_log.iter().map(|x| x.harvested).sum::<u64>(),
            )));

            for owner in g.harvest_log.iter().rev() {
                blocks.push(comment(format!(
                    "{}gp harvested for <@{}>",
                    owner.harvested, owner.id
                )));
            }
        }

        if let (Credentials::None, true) = (credentials, interactivity.market(*credentials)) {
            blocks.push(json!({ "type": "divider" }));
            blocks.push(comment(
                "In order to buy this, you have to have a \
                <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>.",
            ));
        }

        blocks
    }
}

async fn update_user_home_tab(user_id: String) -> Result<(), String> {
    update_home_tab(
        Hacksteader::from_db(&dyn_db(), user_id.clone()).await.ok(),
        user_id.clone(),
    )
    .await
}
async fn update_home_tab(hs: Option<Hacksteader>, user_id: String) -> Result<(), String> {
    let o = json!({
        "user_id": user_id,
        "view": {
            "type": "home",
            "blocks": hacksteader_greeting_blocks(hs, Interactivity::Write, Credentials::Owner),
        }
    });

    debug!("home screen: {}", serde_json::to_string_pretty(&o).unwrap());

    let client = reqwest::Client::new();
    client
        .post("https://slack.com/api/views.publish")
        .bearer_auth(&*TOKEN)
        .json(&o)
        .send()
        .await
        .map_err(|e| format!("couldn't publish home tab view: {}", e))?;

    Ok(())
}

fn progress_bar(size: usize, progress_ratio: f32) -> String {
    format!(
        "`\u{2062}{}\u{2062}`",
        (0..size)
            .map(|i| {
                if (i as f32 / size as f32) < progress_ratio {
                    '\u{2588}'
                } else {
                    ' '
                }
            })
            .collect::<String>()
    )
}

fn hackstead_blocks(
    hs: Hacksteader,
    interactivity: Interactivity,
    credentials: Credentials,
) -> Vec<Value> {
    use humantime::format_duration;
    use std::time::SystemTime;

    // TODO: with_capacity optimization
    let mut blocks: Vec<Value> = Vec::new();

    let neighbor_bonuses = hs.neighbor_bonuses();
    let Hacksteader {
        profile,
        mut inventory,
        land,
        user_id,
        gotchis,
        ..
    } = hs;

    let inv_occurrences = inventory_occurences(inventory.clone());

    //let bottom_gotchi = gotchis.len() < 5;
    //let bottom_inventory = inv_occurrences.len() < 5;
    let hs_adv = profile.current_advancement();
    let next_hs_adv = profile.next_advancement();
    let hs_adv_sum = profile.advancements_sum();

    blocks.push(json!({
        "type": "section",
        "text": mrkdwn(format!(
            "*_<@{}>'s {}_* - *{}lvl* - _{}xp_",
            user_id,
            hs_adv.achiever_title,
            profile.advancements.current_position(profile.xp),
            profile.xp,
        )),
    }));

    blocks.push(comment(format!(
        "founded {} ago (roughly)",
        format_duration(SystemTime::now().duration_since(profile.joined).unwrap()),
    )));
    if let Some(na) = next_hs_adv {
        blocks.push({
            let (have, need) = (profile.xp - hs_adv_sum.xp, na.xp);
            json!({
                "type": "section",
                "text": mrkdwn(format!(
                    "Next: *{}*\n{}  {}xp to go\n_{}_",
                    na.title,
                    progress_bar(50, have as f32 / need as f32),
                    need - have,
                    na.description
                )),
            })
        });
    }
    blocks.push(comment(format!("Last Advancement: \"{}\"", hs_adv.title)));
    blocks.push(comment(format!(
        concat!(
            "The level of your hackstead allows you to redeem ",
            "Land Deeds for up to {} more pieces of land.",
        ),
        hs_adv_sum.land
    )));

    blocks.push(json!({ "type": "divider" }));

    /*if !bottom_inventory {
        let mut actions = vec![];

        /*
        if !bottom_gotchi {
            actions.push(json!({
                "type": "button",
                "text": plain_text("Hackagotchi"),
                "style": "primary",
                "value": serde_json::to_string(&(&user_id, interactivity, credentials, false)).unwrap(),
                "action_id": "gotchi_overview",
            }));
        }*/

        if !bottom_inventory {
            actions.push(json!({
                "type": "button",
                "text": plain_text("Inventory"),
                "style": "primary",
                "value": serde_json::to_string(&(&user_id, interactivity, credentials, false)).unwrap(),
                "action_id": "inventory_overview",
            }));
        }

        blocks.push(json!({
            "type": "actions",
            "elements": actions
        }));

        blocks.push(json!({ "type": "divider" }));
    }*/

    let tiles_owned = land.len();
    for tile in land.into_iter() {
        if let Some(p) = tile.plant.as_ref() {
            let neighbor_bonuses = neighbor_bonuses
                .clone()
                .bonuses_for_plant(tile.id, p.archetype_handle);
            let sum = p.advancements_sum(neighbor_bonuses.iter());
            let unboosted_sum = p.neighborless_advancements_sum(std::iter::empty());
            let ca = p.current_advancement();

            blocks.push(json!({
                "type": "section",
                "text": mrkdwn({
                    let mut s = String::new();
                    s.push_str(&format!(
                        "*{}* - _{}_ - *{}lvl* - {}xp {}\n\n",
                        p.name,
                        ca.achiever_title,
                        p.advancements.current_position(p.xp),
                        p.xp,
                        p
                            .effects
                            .iter()
                            .filter_map(|e| Some(emojify(
                                &CONFIG
                                    .possession_archetypes
                                    .get(e.item_archetype_handle)?
                                    .name
                            )))
                            .collect::<Vec<String>>()
                            .join("")
                    ));
                    if let Some(na) = p.next_advancement() {
                        let (have, need) = (p.xp - unboosted_sum.xp, na.xp);
                        s.push_str(&format!(
                            "Next: *{}*\n{}  {}xp to go\n_{}_",
                            na.title,
                            progress_bar(35, have as f32/need as f32),
                            need - have,
                            na.description
                        ));
                    }
                    s
                }),
                "accessory": {
                    "type": "image",
                    "image_url": format!("http://{}/gotchi/img/plant/{}.gif", *URL, filify(&ca.art)),
                    "alt_text": format!("A healthy, growing {}!", p.name),
                }
            }));
            if let (false, Some(base_yield_duration)) =
                (sum.yields.is_empty(), p.base_yield_duration)
            {
                blocks.push(json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "*Yield*\n{}  {:.3} minutes to go",
                        progress_bar(30, 1.0 - p.until_yield/base_yield_duration),
                        (p.until_yield / sum.yield_speed_multiplier) / FARM_CYCLES_PER_MIN as f32
                    )),
                    "accessory": {
                        "type": "button",
                        "text": plain_text("Yield Stats"),
                        "value": serde_json::to_string(&(&user_id, tile.id)).unwrap(),
                        "action_id": "yield_stats",
                    }
                }));
            }
            if !sum.recipes.is_empty() {
                if let (Some(craft), Some(recipe)) = (p.craft.as_ref(), p.current_recipe()) {
                    blocks.push(json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "*Crafting {}*\n{}  {:.3} minutes to go",
                            recipe.title(),
                            progress_bar(30, 1.0 - craft.until_finish/recipe.time),
                            (craft.until_finish / sum.crafting_speed_multiplier) / FARM_CYCLES_PER_MIN as f32
                        ))
                    }));
                } else {
                    blocks.push(json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "*{}/{}* recipes craftable",
                            sum.recipes.iter().filter(|r| r.satisfies(&inventory)).count(),
                            sum.recipes.len()
                        )),
                        "accessory": {
                            "type": "button",
                            "text": plain_text("Crafting"),
                            "value": serde_json::to_string(&(tile.id, &user_id)).unwrap(),
                            "action_id": "crafting",
                        }
                    }));
                }
            }
        } else {
            blocks.push(json!({
                "type": "section",
                "text": mrkdwn("*Empty Land*\nOpportunity Awaits!"),
                "accessory": {
                    "type": "image",
                    "image_url": format!("http://{}/gotchi/img/icon/dirt.png", *URL),
                    "alt_text": "Land, waiting to be monopolized upon!",
                }
            }));
        }
        match tile.plant {
            Some(p) => {
                let ca = p.current_advancement();
                let mut actions = vec![];

                let applicables: Vec<String> =
                    inventory
                        .iter()
                        .cloned()
                        .filter_map(|x| {
                            x.kind.keepsake()?.item_application.as_ref().filter(|a| {
                                a.effects.iter().any(|e| e.keep_plants.allows(&p.name))
                            })?;
                            Some(x.id.to_simple().to_string())
                        })
                        .collect();

                if !applicables.is_empty() && interactivity.write() {
                    actions.push(json!({
                        "type": "button",
                        "text": plain_text("Apply Item"),
                        "style": "primary",
                        "value": serde_json::to_string(&(
                            tile.id.to_simple().to_string(),
                            user_id.clone()
                        )).unwrap(),
                        "action_id": "item_apply",
                    }))
                }
                actions.push(json!({
                    "type": "button",
                    "text": plain_text("Levels"),
                    "value": serde_json::to_string(&(p.archetype_handle, p.xp)).unwrap(),
                    "action_id": "levels",
                }));

                blocks.push(json!({
                    "type": "actions",
                    "elements": actions,
                }));
                blocks.push(comment(format!("Last Advancement: \"{}\"", ca.title)));
            }
            None => {
                let seeds: Vec<Possessed<possess::Seed>> = inventory
                    .iter()
                    .cloned()
                    .filter_map(|p| p.try_into().ok())
                    .collect();

                blocks.push(if seeds.is_empty() {
                    comment(":seedlet: No seeds! See if you can buy some on the /hackmarket")
                } else if let Interactivity::Write = interactivity {
                    json!({
                        "type": "actions",
                        "elements": [{
                            "type": "button",
                            "text": plain_text("Plant Seed"),
                            "style": "primary",
                            "value": tile.id.to_simple().to_string(),
                            "action_id": "seed_plant",
                        }],
                    })
                } else {
                    comment(":seedling: No planting seeds for you! This page is read only.")
                });
            }
        }
    }

    inventory.sort_unstable_by(|a, b| {
        let l = a
            .kind
            .keepsake()
            .and_then(|k| k.unlocks_land.as_ref())
            .map(|lu| lu.requires_xp);
        let r = b
            .kind
            .keepsake()
            .and_then(|k| k.unlocks_land.as_ref())
            .map(|lu| lu.requires_xp);
        match (l, r) {
            (Some(true), Some(false)) => std::cmp::Ordering::Less,
            (Some(false), Some(true)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    });
    if let Some(land_deed) = inventory.iter().find_map(|possession| {
        possession
            .kind
            .keepsake()?
            .unlocks_land
            .as_ref()
            .filter(|cert| {
                if cert.requires_xp {
                    tiles_owned < hs_adv_sum.land.try_into().unwrap()
                } else {
                    true
                }
            })
            .map(|_| possession)
    }) {
        blocks.push(json!({ "type": "divider" }));

        blocks.push(json!({
            "type": "actions",
            "elements": [{
                "type": "button",
                "text": plain_text("Redeem Land Deed"),
                "style": "primary",
                "value": land_deed.id.to_simple().to_string(),
                "action_id": "unlock_land",
            }],
        }));
    }

    //if bottom_inventory {
    blocks.push(json!({ "type": "divider" }));

    if inv_occurrences.is_empty() {
        blocks.push(comment("Your inventory is empty"));
    } else {
        blocks.append(&mut inventory_section(
            inv_occurrences,
            interactivity,
            credentials,
            false,
            user_id.clone(),
        ));
    }
    //}

    //if bottom_gotchi && gotchis.len() > 0 {
    if gotchis.len() > 0 {
        blocks.push(json!({ "type": "divider" }));

        blocks.append(&mut gotchi_section(
            gotchis,
            interactivity,
            credentials,
            false,
        ));
    }

    if let Interactivity::Read = interactivity {
        blocks.push(json!({ "type": "divider" }));

        blocks.push(comment(format!(
            "This is a read-only snapshot of <@{}>'s Hackagotchi Hackstead at a specific point in time. \
            You can manage your own Hackagotchi Hackstead in real time at your \
            <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>.",
            &user_id,
            *APP_ID,
        )));
    }

    debug!(
        "{}",
        serde_json::to_string_pretty(&json!( { "blocks": blocks.clone() })).unwrap()
    );

    blocks
}

macro_rules! hacksteader_opening_blurb { () => { format!(
"
*Build Your Own Hackstead With Hackagotchi!*


:ear_of_rice: *Grow plants* to *produce and craft* cool items!

:adorpheus: Have a chance to *hatch Hackagotchi* that can help you *earn money* and *boost crops*!

:money_with_wings: *Buy and barter* with other Hack Clubbers on a *real-time market*!


_Hacksteading is *free*!_

Each Hacksteader starts off with a :dirt: *single plot of land* to grow crops \
and a :nest_egg: *Nest Egg* to get started! \
Grow your Hacksteading empire by starting today!
",
) } }

fn hackstead_explanation_blocks() -> Vec<Value> {
    vec![
        json!({
            "type": "section",
            "text": mrkdwn(hacksteader_opening_blurb!()),
        }),
        json!({
            "type": "actions",
            "elements": [{
                "type": "button",
                "action_id": "hackstead_confirm",
                "style": "danger",
                "text": plain_text("Monopolize on Adorableness?"),
                "confirm": {
                    "style": "danger",
                    "title": plain_text("Let's Hackstead, Fred!"),
                    "text": mrkdwn(
                         "(P.S. once you click that button, \
                         expect a direct message from banker on what to do next!)"
                    ),
                    "deny": plain_text("I'm short on GP"),
                    "confirm": plain_text("LET'S HACKSTEAD, FRED!"),
                }
            }]
        }),
    ]
}

/// Returns Slack JSON displaying someone's hackstead if they're
/// registered, if not, this command will greet them with an explanation
/// of what hacksteading is and how they can get a hackstead of their own.
fn hacksteader_greeting_blocks(
    hacksteader: Option<Hacksteader>,
    interactivity: Interactivity,
    creds: Credentials,
) -> Vec<Value> {
    let o = match hacksteader {
        Some(hs) => hackstead_blocks(hs, interactivity, creds),
        None => hackstead_explanation_blocks(),
    };

    debug!("{}", serde_json::to_string_pretty(&o).unwrap());

    o
}

async fn hackmarket_blocks(cat: Category, viewer: String) -> Vec<Value> {
    use config::ArchetypeHandle;

    let sales = market::market_search(&dyn_db(), cat)
        .await
        .map_err(|e| error!("couldn't search market: {}", e))
        .unwrap_or_default();

    let (all_goods_count, all_goods_price) =
        (sales.len(), sales.iter().map(|(s, _)| s.price).sum::<u64>());

    let (your_goods_count, your_goods_price) = sales
        .iter()
        .filter(|(_, p)| p.steader == viewer)
        .map(|(s, _)| s.price)
        .fold((0, 0), |(n, sum), p| (n + 1, sum + p));

    // things for sale, sorted by the type of thing they are.
    let entries: Vec<(String, (u64, usize))> = {
        let mut entries: HashMap<String, (u64, usize, ArchetypeHandle)> = Default::default();

        for (sale, p) in sales.into_iter() {
            entries
                .entry(sale.market_name.clone())
                .and_modify(|e| {
                    if sale.price < e.0 {
                        e.0 = sale.price
                    }
                    e.1 += 1;
                })
                .or_insert((sale.price, 1, p.archetype_handle));
        }

        let mut v: Vec<_> = entries.into_iter().collect();
        v.sort_by_key(|&(_, (_, _, ah))| ah);
        v.into_iter()
            .map(|(name, (lowest_price, count, _))| (name, (lowest_price, count)))
            .collect()
    };

    let entry_count = entries.len();
    std::iter::once(comment(format!(
        concat!(
            "Your *{}* goods cost *{}gp* in total, ",
            "*{}%* of the market's ",
            "_{}gp_ value across _{}_ items.",
        ),
        your_goods_count,
        your_goods_price,
        your_goods_price as f32 / all_goods_price as f32 * 100.0,
        all_goods_price,
        all_goods_count,
    )))
    .chain(
        entries
            .into_iter()
            .flat_map(|(name, (lowest_price, count))| {
                std::iter::once(json!({
                    "type": "section",
                    "fields": [
                        mrkdwn(format!(
                            "{} _{}_",
                            emojify(&name),
                            name,
                        )),
                    ],
                    "accessory": {
                        "type": "button",
                        "style": "primary",
                        "text": plain_text(format!(
                            "{} for sale starting at {}gp",
                            count,
                            lowest_price
                        )),
                        "action_id": "possession_market_overview_page",
                        "value": serde_json::to_string(&(name, cat)).unwrap(),
                    }
                }))
                .chain(std::iter::once(json!({ "type": "divider" })))
            })
            .take((entry_count * 2).saturating_sub(1)),
    )
    .collect()
}

#[derive(FromForm, Debug, Clone)]
struct SlashCommand {
    token: String,
    team_id: String,
    team_domain: String,
    channel_id: String,
    channel_name: String,
    user_id: String,
    user_name: String,
    command: String,
    text: String,
    response_url: String,
    trigger_id: String,
}

#[post("/hackmarket", data = "<slash_command>")]
async fn hackmarket<'a>(slash_command: LenientForm<SlashCommand>) -> Result<(), String> {
    info!("{} | {}", slash_command.command, slash_command.text);

    Modal {
        method: "open".to_string(),
        trigger_id: slash_command.trigger_id.clone(),
        callback_id: "hackstreet_modal".to_string(),
        title: "Hackstreet!".to_string(),
        private_metadata: String::new(),
        blocks: hackmarket_blocks(
            match slash_command.text.as_str() {
                "gotchi" | "g" => Category::Gotchi,
                _ => Category::Misc,
            },
            slash_command.user_id.clone(),
        )
        .await,
        submit: None,
    }
    .launch()
    .await?;

    Ok(())
}

pub async fn stateofsteading_blocks() -> Vec<Value> {
    let profiles = hcor::Profile::fetch_all(&dyn_db()).await.unwrap();
    let tiles = hacksteader::Tile::fetch_all(&dyn_db()).await.unwrap();

    struct PlantEntry {
        owner: String,
        seed_from: String,
        level: usize,
    }

    let mut archetype_occurrences: HashMap<config::PlantArchetype, Vec<PlantEntry>> =
        Default::default();

    for tile in tiles.iter() {
        let plant = match &tile.plant {
            Some(plant) => plant,
            None => continue,
        };

        archetype_occurrences
            .entry((**plant).clone())
            .or_default()
            .push(PlantEntry {
                owner: tile.steader.clone(),
                seed_from: plant
                    .pedigree
                    .last()
                    .map(|x| x.id.clone())
                    .unwrap_or("U013STH0TNG".to_string()),
                level: plant.advancements.current_position(plant.xp),
            });
    }

    std::iter::once(json!({
        "type": "section",
        "text": mrkdwn(format!(
            concat!(
                "Total Hacksteaders: *{}*\n",
                "Total Tiles: *{}*\n",
                "{}",
            ),
            profiles.len(),
            tiles.len(),
            archetype_occurrences
                .iter()
                .map(|(plant_archetype, plants)| format!(
                    "*{}* _{}_ plants",
                    plants.len(),
                    plant_archetype.name
                ))
                .collect::<Vec<String>>()
                .join("\n")
        )),
        "accessory": {
            "type": "image",
            "image_url": format!("http://{}/gotchi/img/icon/seedlet.png", *URL),
            "alt_text": "happy shiny better hackstead",
        }
    }))
    .chain(
        archetype_occurrences
            .iter_mut()
            .map(|(plant_archetype, plants)| {
                plants.sort_by_key(|p| p.level);
                json!({
                    "type": "section",
                    "text": mrkdwn(
                        plants
                            .iter_mut()
                            .map(|plant| format!(
                                "<@{}> grows a *{}lvl* _{}_ from <@{}>'s seed",
                                plant.owner,
                                plant.level,
                                plant_archetype.name,
                                plant.seed_from
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    "accessory": {
                        "type": "image",
                        "image_url": format!(
                            "http://{}/gotchi/img/plant/{}.gif",
                            *URL,
                            filify(&plant_archetype.advancements.base.art)
                        ),
                        "alt_text": "happy shiny plant give u stuffs",
                    }
                })
            }),
    )
    .collect()
}

#[post("/stateofsteading", data = "<slash_command>")]
async fn stateofsteading<'a>(slash_command: LenientForm<SlashCommand>) -> Result<(), String> {
    Modal {
        method: "open".to_string(),
        trigger_id: slash_command.trigger_id.clone(),
        callback_id: "stateofsteading_modal".to_string(),
        title: "All the Steaders!".to_string(),
        private_metadata: String::new(),
        blocks: stateofsteading_blocks().await,
        submit: None,
    }
    .launch()
    .await?;

    Ok(())
}

#[post("/hgive", data = "<slash_command>")]
async fn hgive<'a>(slash_command: LenientForm<SlashCommand>) -> Json<Value> {
    use regex::Regex;

    fn res<S: std::string::ToString>(s: S) -> Json<Value> {
        info!("{}", s.to_string());
        Json(json!({
            "blocks": [{
                "type": "section",
                "text": mrkdwn(s),
            }],
            "response_type": "ephemeral",
        }))
    }

    lazy_static::lazy_static!(
        static ref HGIVE: Regex = Regex::new("<@([A-z0-9]+)\\|.+>( [0-9]+)? :(.+):").unwrap();
    );

    info!("trying /give {}", slash_command.text);
    let c = match HGIVE.captures(&slash_command.text) {
        Some(c) => c,
        None => return res("Invalid syntax!"),
    };
    info!("{:?}", c);
    let receiver = match c.get(1) {
        Some(s) => s.as_str().to_string(),
        None => return res("Couldn't parse receiver?"),
    };
    let amount = c
        .get(2)
        .and_then(|x| x.as_str().trim().parse().ok())
        .unwrap_or(1);
    let possession_name = match c.get(3) {
        Some(a) => a.as_str().replace("_", " ").to_lowercase(),
        None => return res("Couldn't parse possession name?"),
    };
    let (archetype_handle, possession_archetype) = match CONFIG
        .possession_archetypes
        .iter()
        .enumerate()
        .find(|(_, x)| x.name.to_lowercase() == possession_name)
    {
        Some(ah) => ah,
        None => return res(format!("no possession by name of {}", possession_name)),
    };

    let user = slash_command.user_id.to_string();
    let hs = match Hacksteader::from_db(&dyn_db(), user.clone()).await {
        Ok(hs) => hs,
        Err(_) => {
            return res(format!(
                concat!(
                    "Can't give {}; ",
                    "you don't have a hackstead! ",
                    "try /hstead to get started!"
                ),
                slash_command.text
            ))
        }
    };

    let possessions = hs
        .inventory
        .into_iter()
        .filter(|p| p.archetype_handle == archetype_handle)
        .take(amount)
        .collect::<Vec<possess::Possession>>();

    if possessions.len() != amount {
        return match possessions.len() {
            0 => res(format!("You don't have any {}", possession_name)),
            more => res(format!(
                "You only have {} {}, not {}!",
                more, possession_name, amount
            )),
        };
    } else if amount == 0 {
        res("Well, I mean ... that's not really anything but ... ok")
    } else {
        let res_msg = json!({
            "blocks": [
                {
                    "type": "section",
                    "text": mrkdwn(format!(
                        "<@{}> shall soon happen across *{}* of <@{}>'s :{}: _{}_!",
                        receiver,
                        amount,
                        user,
                        possession_name.replace(" ", "_"),
                        possession_archetype.name,
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!(
                            "http://{}/gotchi/img/misc/{}.png",
                            *URL,
                            filify(&possession_name)
                        ),
                        "alt_text": "hackagotchi img",
                    }
                },
                comment("TAKE THIS AND DONT TELL MOM")
            ],
            "response_type": "in_channel",
        });

        tokio::spawn(async move {
            info!("I mean this happens?");

            let _ = gift_dm(&user, &receiver, possessions.first().unwrap(), amount)
                .await
                .map_err(|e| error!("{}", e));

            for possession in possessions {
                match Hacksteader::transfer_possession(
                    &dyn_db(),
                    receiver.clone(),
                    possess::Acquisition::Trade,
                    Key::misc(possession.id),
                )
                .await
                {
                    Err(e) => {
                        let _ = banker::message(e).await.map_err(|e| error!("{}", e));
                    }
                    Ok(_) => {}
                }
            }
        });

        Json(res_msg)
    }
}

#[post("/egghatchwhen", data = "<slash_command>")]
async fn egghatchwhen<'a>(
    slash_command: LenientForm<SlashCommand>,
    to_farming: State<'_, Sender<FarmingInputEvent>>,
) -> Json<Value> {
    use rand::seq::SliceRandom;
    let SlashCommand { text, user_id, .. } = (*slash_command).clone();

    if text != "" {
        return Json(json!({
            "response_type": "ephemeral",
        }));
    }

    fn res<S: std::string::ToString>(s: S) -> Json<Value> {
        Json(json!({
            "blocks": [{
                "type": "section",
                "text": mrkdwn(s),
            }],
            "response_type": "in_channel",
        }))
    }

    let user = user_id.to_string();
    let hs = match Hacksteader::from_db(&dyn_db(), user.clone()).await {
        Ok(hs) => hs,
        Err(_) => {
            return res(concat!(
                "Can't open one of your eggs; ",
                "you don't have a hackstead! ",
                "try /hstead to get started!"
            ))
        }
    };

    let egg_search = hs
        .gotchis
        .into_iter()
        .filter(|p| p.inner.hatch_table.is_some())
        .map(|p| p.id)
        .collect::<Vec<uuid::Uuid>>()
        .choose(&mut rand::thread_rng())
        .map(|id| id.clone());
    let egg_id = match egg_search {
        None => return res("You don't have any eggs to hatch!"),
        Some(id) => id,
    };

    to_farming
        .send(FarmingInputEvent::ActivateUser(user.clone()))
        .unwrap();

    to_farming
        .send(FarmingInputEvent::HatchEgg(egg_id, user.clone()))
        .unwrap();

    res("Selected one of your eggs and hatched it!")
}
#[post("/hackstead", data = "<slash_command>")]
async fn hackstead<'a>(slash_command: LenientForm<SlashCommand>) -> Json<Value> {
    debug!("{:#?}", slash_command);

    lazy_static::lazy_static! {
        static ref HACKSTEAD: Regex = Regex::new(
            "(<@([A-z0-9]+)|(.+)>)?"
        ).unwrap();
    }

    let captures = HACKSTEAD.captures(&slash_command.text);
    debug!("captures: {:#?}", captures);
    let user = captures
        .and_then(|c| c.get(2).map(|x| x.as_str()))
        .unwrap_or(&slash_command.user_id);

    let hs = Hacksteader::from_db(&dyn_db(), user.to_string()).await;
    Json(json!({
        "blocks": hacksteader_greeting_blocks(
            hs.ok(),
            Interactivity::Read,
            Credentials::None
        ),
        "response_type": "ephemeral",
    }))
}

#[derive(FromForm, Debug)]
struct ActionData {
    payload: String,
}
#[derive(serde::Deserialize, Debug)]
pub struct Interaction {
    trigger_id: String,
    actions: Vec<Action>,
    user: User,
    view: Option<View>,
}
#[derive(serde::Deserialize, Debug)]
pub struct View {
    private_metadata: String,
    callback_id: String,
    root_view_id: String,
}
#[derive(serde::Deserialize, Debug)]
pub struct User {
    id: String,
}
#[derive(serde::Deserialize, Debug)]
pub struct Action {
    action_id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    value: String,
}

#[derive(Default)]
pub struct Modal {
    method: String,
    trigger_id: String,
    callback_id: String,
    title: String,
    private_metadata: String,
    blocks: Vec<Value>,
    submit: Option<String>,
}

impl Modal {
    async fn launch(self) -> Result<Value, String> {
        let mut o = json!({
            "trigger_id": self.trigger_id,
            "view": {
                "type": "modal",
                "private_metadata": self.private_metadata,
                "callback_id": self.callback_id,
                "title": plain_text(self.title),
                "blocks": self.blocks
            }
        });

        if let Some(submit_msg) = self.submit {
            o["view"]
                .as_object_mut()
                .unwrap()
                .insert("submit".to_string(), plain_text(submit_msg));
        }

        let client = reqwest::Client::new();
        client
            .post(&format!("https://slack.com/api/views.{}", self.method))
            .bearer_auth(&*TOKEN)
            .json(&o)
            .send()
            .await
            .map_err(|e| format!("couldn't open modal: {}", e))?;

        debug!("{}", serde_json::to_string_pretty(&o).unwrap());
        Ok(o)
    }
}

#[derive(Default)]
pub struct ModalUpdate {
    trigger_id: String,
    callback_id: String,
    title: String,
    private_metadata: String,
    hash: Option<String>,
    view_id: String,
    blocks: Vec<Value>,
    submit: Option<String>,
}

impl ModalUpdate {
    async fn launch(self) -> Result<Value, String> {
        let mut o = json!({
            "trigger_id": self.trigger_id,
            "view_id": self.view_id,
            "view": {
                "type": "modal",
                "private_metadata": self.private_metadata,
                "callback_id": self.callback_id,
                "title": plain_text(self.title),
                "blocks": self.blocks
            }
        });

        if let Some(hash) = self.hash {
            o.as_object_mut()
                .unwrap()
                .insert("hash".to_string(), json!(hash));
        }

        if let Some(submit_msg) = self.submit {
            o["view"]
                .as_object_mut()
                .unwrap()
                .insert("submit".to_string(), plain_text(submit_msg));
        }

        let client = reqwest::Client::new();
        client
            .post("https://slack.com/api/views.update")
            .bearer_auth(&*TOKEN)
            .json(&o)
            .send()
            .await
            .map_err(|e| format!("couldn't open modal: {}", e))?;

        debug!("{}", serde_json::to_string_pretty(&o).unwrap());
        Ok(o)
    }
}

#[derive(rocket::Responder)]
pub enum ActionResponse {
    Json(Json<Value>),
    Ok(()),
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Interactivity {
    Read,
    Write,
    Buy,
}
impl Interactivity {
    fn id(self) -> &'static str {
        use Interactivity::*;
        match self {
            Read => "static",
            Write => "dynamic",
            Buy => "market",
        }
    }
}
impl Interactivity {
    fn market(&self, creds: Credentials) -> bool {
        match self {
            Interactivity::Buy => true,
            Interactivity::Write => creds == Credentials::Owner,
            _ => false,
        }
    }
    fn write(&self) -> bool {
        match self {
            Interactivity::Write => true,
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Credentials {
    Owner,
    Hacksteader,
    None,
}

#[post("/interact", data = "<action_data>")]
async fn action_endpoint(
    to_farming: State<'_, Sender<FarmingInputEvent>>,
    action_data: LenientForm<ActionData>,
) -> Result<ActionResponse, String> {
    debug!("{:?}", action_data);
    let v = serde_json::from_str::<Value>(&action_data.payload).unwrap();
    debug!("action data: {:#?}", v);

    if let Some("view_submission") = v.get("type").and_then(|t| t.as_str()) {
        debug!("right type!");
        let view = v.get("view").and_then(|view| {
            let parsed_view = serde_json::from_value::<View>(view.clone()).ok()?;
            let key_json = &parsed_view.private_metadata;
            let key: Option<Key> = match serde_json::from_str(&key_json) {
                Ok(k) => Some(k),
                Err(e) => {
                    error!("couldn't parse {}: {}", key_json, e);
                    None
                }
            };
            Some((
                parsed_view,
                key,
                v.get("trigger_id")?.as_str()?,
                view.get("state").and_then(|s| s.get("values").cloned())?,
                serde_json::from_value::<User>(v.get("user")?.clone()).ok()?,
            ))
        });
        if let Some((view, Some(key), trigger_id, values, user)) = view {
            debug!("view state values: {:#?}", values);

            match view.callback_id.as_str() {
                "sale_removal" => {
                    info!("Revoking sale");
                    market::take_off_market(&dyn_db(), key).await?;

                    return Ok(ActionResponse::Json(Json(json!({
                        "response_action": "clear",
                    }))));
                }
                "sale_complete" => {
                    info!("Completing sale!");
                    let possession = hacksteader::get_possession(&dyn_db(), key).await?;

                    if let Some(sale) = possession.sale.as_ref() {
                        banker::invoice(
                            &user.id,
                            sale.price,
                            &format!(
                                "hackmarket purchase buying {} at {}gp :{}:{} from <@{}>",
                                possession.name,
                                sale.price,
                                key.id,
                                key.category as u8,
                                possession.steader,
                            ),
                        )
                        .await?;
                    }

                    return Ok(ActionResponse::Json(Json(json!({
                        "response_action": "clear",
                    }))));
                }
                _ => {}
            };

            if let Some(Value::String(nickname)) = values
                .get("gotchi_nickname_block")
                .and_then(|i| i.get("gotchi_nickname_input"))
                .and_then(|s| s.get("value"))
            {
                // update the nickname in the DB
                let db = dyn_db();
                db.update_item(rusoto_dynamodb::UpdateItemInput {
                    table_name: hcor::TABLE_NAME.to_string(),
                    key: key.into_item(),
                    update_expression: Some("SET nickname = :new_name".to_string()),
                    expression_attribute_values: Some(
                        [(
                            ":new_name".to_string(),
                            AttributeValue {
                                s: Some(nickname.clone()),
                                ..Default::default()
                            },
                        )]
                        .iter()
                        .cloned()
                        .collect(),
                    ),
                    ..Default::default()
                })
                .await
                .map_err(|e| format!("Couldn't change nickname in database: {}", e))?;

                // TODO: parse what the above could return
                let mut possession = hacksteader::get_possession(&db, key).await?;

                let gotchi = possession
                    .kind
                    .gotchi_mut()
                    .ok_or("can only nickname gotchi".to_string())?;

                // update the nickname on the Gotchi,
                gotchi.nickname = nickname.clone();

                let page = PossessionPage {
                    credentials: Credentials::Owner,
                    interactivity: Interactivity::Write,
                    possession,
                };

                // update the page in the background with the new gotchi data
                page.modal_update(trigger_id.to_string(), view.root_view_id)
                    .launch()
                    .await?;

                // update the home tab
                to_farming
                    .send(FarmingInputEvent::ActivateUser(user.id.clone()))
                    .unwrap();

                // this will close the "enter nickname" modal
                return Ok(ActionResponse::Ok(()));
            } else if let Some(price) = values
                .get("possession_sell_price_block")
                .and_then(|i| i.get("possession_sell_price_input"))
                .and_then(|s| s.get("value"))
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                let possession = hacksteader::get_possession(&dyn_db(), key).await?;

                banker::invoice(
                    &user.id,
                    price / 20_u64,
                    &format!(
                        "hackmarket fees for selling {} at {}gp :{}:{}",
                        possession.name,
                        price,
                        possession.id,
                        possession.kind.category() as u8
                    ),
                )
                .await?;

                return Ok(ActionResponse::Ok(()));
            } else if let Some(Value::String(new_owner)) = values
                .get("possession_give_receiver_block")
                .and_then(|i| i.get("possession_give_receiver_input"))
                .and_then(|s| s.get("selected_user"))
            {
                info!(
                    "giving {} from {} to {}",
                    view.private_metadata, user.id, new_owner
                );

                if user.id == *new_owner {
                    info!("self giving attempted");

                    return Ok(ActionResponse::Json(Json(json!({
                        "response_action": "errors",
                        "errors": {
                            "possession_give_receiver_input": "absolutely not okay",
                        }
                    }))));
                }

                // update the owner in the DB
                Hacksteader::transfer_possession(
                    &dyn_db(),
                    new_owner.clone(),
                    possess::Acquisition::Trade,
                    key,
                )
                .await?;

                // update the home tab
                // TODO: make this not read from the database
                update_user_home_tab(user.id.clone()).await?;

                let possession = hacksteader::get_possession(&dyn_db(), key).await?;
                let notif_msg =
                    format!("<@{}> has gifted you a {}!", user.id, possession.nickname())
                        .to_string();

                // DM the new_owner about their new acquisition!
                gift_dm(&user.id, new_owner, &possession, 1).await?;

                // close ALL THE MODALS!!!
                return Ok(ActionResponse::Json(Json(json!({
                    "response_action": "clear",
                }))));
            }
        } else if let Some((view, None, _trigger_id, values, user)) = view {
            debug!("view state values: {:#?}", values);

            match view.callback_id.as_str() {
                "crafting_confirm_modal" => {
                    info!("crafting confirm modal");

                    let (tile_id, recipe_archetype_handle): (uuid::Uuid, config::ArchetypeHandle) =
                        serde_json::from_str(&view.private_metadata)
                            .map_err(|e| error!("{}", e))
                            .unwrap();

                    to_farming
                        .send(FarmingInputEvent::BeginCraft {
                            tile_id,
                            recipe_archetype_handle,
                        })
                        .expect("couldn't send to farming");

                    return Ok(ActionResponse::Json(Json(json!({
                        "response_action": "clear",
                    }))));
                }
                _ => {}
            };

            if let Some((tile_id, seed_id)) = values
                .get("seed_plant_input")
                .and_then(|i| i.get("seed_plant_select"))
                .and_then(|s| s.get("selected_option"))
                .and_then(|s| s.get("value"))
                .and_then(|s| s.as_str())
                .and_then(|v| serde_json::from_str(v).ok())
            {
                info!("planting seed!");
                let db = dyn_db();
                let seed = Hacksteader::take(&db, Key::misc(seed_id))
                    .await
                    .map_err(|e| {
                        let a = format!("couldn't delete seed: {}", e);
                        error!("{}", a);
                        a
                    })?
                    .try_into()
                    .map_err(|e| {
                        let a = format!("seed_id wrong type: {}", e);
                        error!("{}", a);
                        a
                    })?;

                to_farming
                    .send(FarmingInputEvent::PlantSeed(
                        tile_id,
                        hacksteader::Plant::from_seed(seed),
                    ))
                    .unwrap();

                to_farming
                    .send(FarmingInputEvent::ActivateUser(user.id.clone()))
                    .unwrap();

                update_user_home_tab(user.id).await.map_err(|e| {
                    let a = format!("{}", e);
                    error!("{}", a);
                    a
                })?;

                return Ok(ActionResponse::Ok(()));
            }
            if let Some((tile_id, item_id)) = values
                .get("item_apply_input")
                .and_then(|i| i.get("item_apply_select"))
                .and_then(|s| s.get("selected_option"))
                .and_then(|s| s.get("value"))
                .and_then(|s| s.as_str())
                .and_then(|v| serde_json::from_str(v).ok())
            {
                info!("applying item!");

                to_farming
                    .send(FarmingInputEvent::ApplyItem(
                        ItemApplication {
                            tile: tile_id,
                            item: item_id,
                        },
                        user.id.clone(),
                    ))
                    .unwrap();

                return Ok(ActionResponse::Ok(()));
            }
        }
    }

    let mut i: Interaction = serde_json::from_str(&action_data.payload).map_err(|e| {
        let a = format!("bad data: {}", e);
        error!("{}", a);
        a
    })?;

    debug!("{:#?}", i);

    let action = i.actions.pop().ok_or_else(|| "no action?".to_string())?;

    let output_json = match action
        .action_id
        .or(action.name.clone())
        .ok_or("no action name".to_string())?
        .as_str()
    {
        "hackstead_confirm" => {
            info!("confirming new user!");
            if !hacksteader::exists(&dyn_db(), i.user.id.clone()).await {
                banker::invoice(&i.user.id, *HACKSTEAD_PRICE, "let's hackstead, fred!")
                    .await
                    .map_err(|e| format!("couldn't send Banker invoice DM: {}", e))?;
            }

            mrkdwn("Check your DMs from Banker for the hacksteading invoice!")
        }
        "possession_sell" => {
            let page_json = i.view.ok_or("no view!".to_string())?.private_metadata;
            //let page: PossessionPage = serde_json::from_str(&page_json)
            // .map_err(|e| dbg!(format!("couldn't parse {}: {}", page_json, e)))?;

            Modal {
                method: "push".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "possession_sell_modal".to_string(),
                title: "Sell Item".to_string(),
                private_metadata: page_json,
                blocks: vec![
                    json!({
                        "type": "input",
                        "block_id": "possession_sell_price_block",
                        "label": plain_text("Price (gp)"),
                        "element": {
                            "type": "plain_text_input",
                            "action_id": "possession_sell_price_input",
                            "placeholder": plain_text("Price Item"),
                            "initial_value": "50",
                        }
                    }),
                    json!({ "type": "divider" }),
                    comment("As a form of confirmation, you'll get an invoice to pay before your Item goes up on the market. \
                        To fund Harvests and to encourage Hacksteaders to keep prices sensible, \
                        this invoice is 5% of the price of your sale \
                        rounded down to the nearest GP (meaning that sales below 20gp aren't taxed at all)."),
                ],
                submit: Some("Sell!".to_string()),
                ..Default::default()
            }
            .launch()
            .await?
        }
        "possession_give" => {
            let key_json = i.view.ok_or("no view!".to_string())?.private_metadata;
            let key: Key = serde_json::from_str(&key_json).map_err(|e| {
                let a = format!("couldn't parse {}: {}", key_json, e);
                error!("{}", a);
                a
            })?;

            let possession = hacksteader::get_possession(&dyn_db(), key).await?;

            Modal {
                method: "push".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "possession_give_modal".to_string(),
                title: "Give Item".to_string(),
                blocks: vec![json!({
                    "type": "input",
                    "block_id": "possession_give_receiver_block",
                    "label": plain_text("Give Item"),
                    "element": {
                        "type": "users_select",
                        "action_id": "possession_give_receiver_input",
                        "placeholder": plain_text("Who Really Gets your Gotchi?"),
                        "initial_user": ({
                            let s = &CONFIG.special_users;
                            &s.get(key_json.len() % s.len()).unwrap_or(&*ID)
                        }),
                        "confirm": {
                            "title": plain_text("You sure?"),
                            "text": mrkdwn(format!(
                                "Are you sure you want to give away {} _{}_? You might not get them back. :frowning:",
                                emojify(&possession.name),
                                possession.nickname()
                            )),
                            "confirm": plain_text("Give!"),
                            "deny": plain_text("No!"),
                            "style": "danger",
                        }
                    }
                })],
                private_metadata: key_json,
                submit: Some("Trade Away!".to_string()),
                ..Default::default()
            }
            .launch()
            .await?
        }
        "unlock_land" => {
            // id of the item which allowed them to unlock this land
            let cert_id: uuid::Uuid = uuid::Uuid::parse_str(&action.value).map_err(|e| {
                let a = format!("couldn't parse land cert id: {}", e);
                error!("{}", a);
                a
            })?;

            to_farming
                .send(FarmingInputEvent::RedeemLandCert(
                    cert_id,
                    i.user.id.clone(),
                ))
                .unwrap();

            json!({})
        }
        "seed_plant" => {
            let tile_id: uuid::Uuid = uuid::Uuid::parse_str(&action.value).unwrap();
            let hs = match Hacksteader::from_db(&dyn_db(), i.user.id.clone()).await {
                Ok(hs) => hs,
                Err(e) => {
                    let a = format!("error fetching user for seed plant: {}", e);
                    error!("{}", a);
                    return Err(a);
                }
            };
            let seeds: Vec<Possessed<possess::Seed>> = hs
                .inventory
                .iter()
                .cloned()
                .filter_map(|p| p.try_into().ok())
                .collect();

            Modal {
                method: "open".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "seed_plant_modal".to_string(),
                title: "Plant a Seed!".to_string(),
                private_metadata: String::new(),
                blocks: vec![json!({
                    "type": "input",
                    "label": plain_text("Seed Select"),
                    "block_id": "seed_plant_input",
                    "element": {
                        "type": "static_select",
                        "placeholder": plain_text("Which seed do ya wanna plant?"),
                        "action_id": "seed_plant_select",
                        // show them each seed they have that grows a given plant
                        "option_groups": CONFIG
                            .plant_archetypes
                            .iter()
                            .filter_map(|pa| { // plant archetype
                                let mut seed_iter = seeds
                                    .iter()
                                    .filter(|s| s.inner.grows_into == pa.name);
                                let first_seed = seed_iter.next();
                                // technically a lie if first_seed is None
                                let seed_count = seed_iter.count() + 1;

                                if let Some(s) = first_seed {
                                    let mut desc = format!(
                                        "{} - {}",
                                        seed_count,
                                        s.description
                                    );
                                    desc.truncate(75);
                                    if dbg!(desc.len()) == 75 {
                                        desc.truncate(71);
                                        desc.push_str("...")
                                    }

                                    Some(json!({
                                        "label": plain_text(&pa.name),
                                        "options": [{
                                            "text": plain_text(format!("{} {}", emojify(&s.name), s.name)),
                                            "description": plain_text(desc),
                                            // this is fucky-wucky because value can only be 75 chars
                                            "value": serde_json::to_string(&(
                                                &tile_id.to_simple().to_string(),
                                                s.id.to_simple().to_string(),
                                            )).unwrap(),
                                        }]
                                    }))
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<Value>>(),
                    }
                })],
                submit: Some("Plant it!".to_string()),
            }
            .launch()
            .await?
        }
        "gotchi_overview" => {
            let (steader, interactivity, credentials, push): (
                String,
                Interactivity,
                Credentials,
                bool,
            ) = serde_json::from_str(&action.value).unwrap();

            let hs = Hacksteader::from_db(&dyn_db(), steader).await?;

            let gotchi_count = hs.gotchis.len();
            let blocks = gotchi_section(hs.gotchis, interactivity, credentials, true);

            Modal {
                method: if push { "push" } else { "open" }.to_string(),
                trigger_id: i.trigger_id,
                callback_id: "gotchi_overview_modal".to_string(),
                title: match gotchi_count {
                    1 => "Your Hackagotchi".to_string(),
                    more => format!("Your {} Hackagotchi", more),
                },
                private_metadata: action.value.to_string(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "inventory_overview" => {
            let (steader, interactivity, credentials, push): (
                String,
                Interactivity,
                Credentials,
                bool,
            ) = serde_json::from_str(&action.value).unwrap();

            let hs = Hacksteader::from_db(&dyn_db(), steader.clone()).await?;

            let inv_count = hs.inventory.len();
            let blocks = inventory_section(
                inventory_occurences(hs.inventory),
                interactivity,
                credentials,
                true,
                steader.clone(),
            );

            Modal {
                method: if push { "push" } else { "open" }.to_string(),
                trigger_id: i.trigger_id,
                callback_id: "inventory_overview_modal".to_string(),
                title: match inv_count {
                    1 => "Your Item".to_string(),
                    more => format!("Your {} Items", more),
                },
                private_metadata: action.value.to_string(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "crafting_confirm" => {
            let craft_json = &action.value;
            let (plant_id, recipe_index): (uuid::Uuid, config::ArchetypeHandle) =
                serde_json::from_str(&craft_json).unwrap();

            let hs = Hacksteader::from_db(&dyn_db(), i.user.id.clone()).await?;
            let all_nb = hs.neighbor_bonuses();
            let plant = hs
                .land
                .into_iter()
                .find(|t| t.id == plant_id)
                .ok_or_else(|| {
                    let e = format!("no tile with id {} for this user {} ", plant_id, i.user.id);
                    error!("{}", e);
                    e
                })?
                .plant
                .ok_or_else(|| {
                    let e = format!("can't craft on tile {}; it's not a plant", plant_id);
                    error!("{}", e);
                    e
                })?;

            let neighbor_bonuses = all_nb.bonuses_for_plant(plant_id, plant.archetype_handle);

            let sum = plant.advancements_sum(neighbor_bonuses.iter());

            let recipe = plant.get_recipe(recipe_index).ok_or_else(|| {
                let e = format!(
                    "can't craft unknown recipe: {} on {:?} {}xp",
                    recipe_index, plant.name, plant.xp
                );
                error!("{}", e);
                e
            })?;
            let possible_output = recipe.makes.any();

            Modal {
                method: "push".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "crafting_confirm_modal".to_string(),
                title: "Crafting Confirmation".to_string(),
                private_metadata: craft_json.to_string(),
                blocks: vec![json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        concat!(
                            "Are you sure you want your plant to ",
                            "spend the next {:.2} minutes crafting {} ",
                            "using\n\n{}\n{}",
                        ),
                        (recipe.time / sum.crafting_speed_multiplier) / FARM_CYCLES_PER_MIN as f32,
                        recipe.makes,
                        recipe
                            .needs
                            .iter()
                            .map(|(n, what)| {
                                format!(
                                    "*{}* {} _{}_",
                                    n,
                                    emojify(&what.name),
                                    what.name
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        if recipe.destroys_plant {
                            "WARNING: THIS WILL DESTROY YOUR PLANT"
                        } else {
                            ""
                        }
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": match possible_output {
                            Some(po) => format!("http://{}/gotchi/img/{}/{}.png",
                                *URL,
                                po.kind.category(),
                                filify(&po.name)
                            ),
                            None => format!("http://{}/gotchi/img/icon/dirt.png", *URL),
                        },
                        "alt_text": "The thing you'd like to craft",
                    }
                })],
                submit: Some("Craft!".to_string()),
            }
            .launch()
            .await?
        }
        "crafting" => {
            info!("crafting confirmation window");

            let (plant_id, steader): (uuid::Uuid, String) =
                serde_json::from_str(&action.value).unwrap();
            let hs = Hacksteader::from_db(&dyn_db(), i.user.id.to_string()).await?;
            let plant = hs
                .land
                .iter()
                .find_map(|tile| tile.plant.as_ref().filter(|_p| tile.id == plant_id))
                .ok_or_else(|| format!("no such plant!"))?;
            let all_nb = hs.neighbor_bonuses();
            let neighbor_bonuses = all_nb.bonuses_for_plant(plant_id, plant.archetype_handle);

            let recipes = plant.advancements_sum(neighbor_bonuses.iter()).recipes;
            let unlocked_recipes = recipes.len();
            let max_recipes = plant.advancements_max_sum(neighbor_bonuses.iter()).recipes;

            let blocks = recipes
                .into_iter()
                .enumerate()
                .flat_map(|(recipe_handle, raw_recipe)| {
                    use hcor::config::Archetype;

                    let possible = raw_recipe.satisfies(&hs.inventory);
                    let recipe = raw_recipe
                        .clone()
                        .lookup_handles()
                        .expect("invalid archetype handle");
                    let mut b = Vec::with_capacity(recipe.needs.len() + 2);
                    let output: Vec<(&'static Archetype, usize)> = recipe.makes.all();
                    let craft_output_count: usize = output.iter().map(|(_, n)| n).sum();

                    let mut head = json!({
                        "type": "section",
                        "text": mrkdwn(if craft_output_count <= 1 {
                                let (hi, lo) = recipe.xp;
                                format!(
                                    "*{}* + around {}xp\n_{}_",
                                    recipe.title(),
                                    (hi + lo) / 2,
                                    recipe.explanation()
                                )
                            } else {
                                output
                                    .iter()
                                    .map(|(a, n)| format!(
                                        "*{}* {} _{}_",
                                        n,
                                        emojify(&a.name),
                                        a.name,
                                    ))
                                    .collect::<Vec<String>>()
                                    .join(match recipe.makes {
                                        config::RecipeMakes::OneOf(_) => " *or*\n",
                                        _ => " *and*\n",
                                    })
                            }
                        ),
                    });
                    if possible && steader == i.user.id {
                        head.as_object_mut().unwrap().insert(
                            "accessory".to_string(),
                            json!({
                                "type": "button",
                                "style": "primary",
                                "text": plain_text(match craft_output_count {
                                    1 => format!("Craft {}", recipe.title()),
                                    _ => "Craft".to_string()
                                }),
                                "value": serde_json::to_string(&(
                                    &plant_id,
                                    recipe_handle,
                                )).unwrap(),
                                "action_id": "crafting_confirm",
                            }),
                        );
                    }
                    b.push(head);

                    b.push(comment("*needs:* ".to_string()));
                    for (count, resource) in recipe.needs {
                        b.push(comment(format!(
                            "*{}* {} _{}_",
                            count,
                            emojify(&resource.name),
                            resource.name
                        )));
                    }
                    b.push(json!({ "type": "divider" }));

                    b
                })
                .chain(max_recipes.into_iter().skip(unlocked_recipes).map(|r| {
                    comment(format!(
                        "*Level up to unlock:* {}",
                        r.lookup_handles().unwrap().title()
                    ))
                }))
                .collect();

            Modal {
                method: "open".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "crafting_modal".to_string(),
                title: "Crafting".to_string(),
                private_metadata: String::new(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "levels" => {
            let (ah, xp): (config::ArchetypeHandle, u64) =
                serde_json::from_str(&action.value).unwrap();
            let arch = CONFIG
                .plant_archetypes
                .get(ah)
                .ok_or_else(|| format!("invalid archetype handle: {}", ah))?;
            let current_position = arch.advancements.current_position(xp);

            let blocks = arch
                .advancements
                .all()
                .enumerate()
                .map(|(i, adv)| {
                    let text = format!(
                        "*{}* - {} - {}xp\n_{}_",
                        adv.title, adv.achiever_title, adv.xp, adv.description,
                    );

                    if i <= current_position {
                        json!({
                            "type": "section",
                            "text": mrkdwn(text),
                        })
                    } else {
                        comment(text)
                    }
                })
                .collect();

            Modal {
                method: "open".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "levels_modal".to_string(),
                title: "Levels Overview".to_string(),
                private_metadata: String::new(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "yield_stats" => {
            use config::PlantAdvancementKind::*;
            let (user_id, plant_id): (String, uuid::Uuid) =
                serde_json::from_str(&action.value).unwrap();

            let hs = Hacksteader::from_db(&dyn_db(), user_id.to_string()).await?;
            let plant = hs
                .land
                .iter()
                .find_map(|tile| tile.plant.as_ref().filter(|_p| tile.id == plant_id))
                .ok_or_else(|| format!("no such plant!"))?;
            let all_nb = hs.neighbor_bonuses();
            let neighbor_bonuses = all_nb.bonuses_for_plant(plant_id, plant.archetype_handle);

            let advancements = plant
                .unlocked_advancements(neighbor_bonuses.iter())
                .filter(|a| match &a.kind {
                    Neighbor(..) => false,
                    _ => true,
                })
                .chain(neighbor_bonuses.iter())
                .collect::<Vec<_>>();
            let sum = plant.advancements_sum(neighbor_bonuses.iter());
            let yield_farm_cycles = plant
                .base_yield_duration
                .map(|x| x / sum.yield_speed_multiplier);

            let mut blocks = vec![];

            if let Some(yield_farm_cycles) = yield_farm_cycles {
                blocks.push(json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        concat!(
                            "*Yield Speed*\n",
                            "Yields every: *{:.2} minutes*\n",
                            "Total Speedboost: *x{:.3}*",
                        ),
                        yield_farm_cycles / FARM_CYCLES_PER_MIN as f32,
                        sum.yield_speed_multiplier
                    )),
                }));
            }

            for adv in advancements.iter() {
                match &adv.kind {
                    YieldSpeedMultiplier(s) => {
                        blocks.push(comment(format!("_{}_: *x{}* speed boost", adv.title, s)));
                    }
                    Neighbor(s) => match **s {
                        YieldSpeedMultiplier(s) => {
                            blocks.push(comment(format!(
                                "_{}_: *x{}* speed boost _(from neighbor)_",
                                adv.title, s
                            )));
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            blocks.push(json!({
                "type": "section",
                "text": mrkdwn(format!(
                    "*Yield Size*\n*x{:.3}* yield size multiplier",
                    sum.yield_size_multiplier,
                ))
            }));
            for adv in advancements.iter() {
                match &adv.kind {
                    YieldSizeMultiplier(x) => {
                        blocks.push(comment(format!("_{}_: *x{}* size boost", adv.title, x)));
                    }
                    Neighbor(s) => match **s {
                        YieldSizeMultiplier(s) => {
                            blocks.push(comment(format!(
                                "_{}_: *x{}* size boost _(from neighbor)_",
                                adv.title, s
                            )));
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            blocks.push(json!({
                "type": "section",
                "text": mrkdwn("*Yield Items*".to_string())
            }));
            for y in sum.yields.iter() {
                let arch = match CONFIG.possession_archetypes.get(y.yields) {
                    Some(arch) => arch,
                    None => {
                        error!("unknown arch in yield {}", y.yields);
                        continue;
                    }
                };
                let name = &arch.name;

                let (lo, hi) = y.amount;
                blocks.push(comment(format!(
                    "{}between *{}* and *{}* {} _{}_",
                    if y.chance == 1.0 {
                        "".to_string()
                    } else {
                        format!("up to *{:.1}*% chance of ", y.chance * 100.0)
                    },
                    lo.floor(),
                    hi.ceil(),
                    emojify(name),
                    name,
                )));
            }

            Modal {
                method: "open".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "yield_stats_modal".to_string(),
                title: "Yield Stats".to_string(),
                private_metadata: String::new(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "item_apply" => {
            use possess::Keepsake;

            let (tile_id, user_id): (uuid::Uuid, String) = serde_json::from_str(&action.value)
                .map_err(|e| {
                    let a = format!("couldn't parse action value: {}", e);
                    error!("{}", a);
                    a
                })?;
            let db = dyn_db();

            let Hacksteader {
                inventory, land, ..
            } = Hacksteader::from_db(&db, user_id).await.map_err(|e| {
                error!("{}", e);
                e
            })?;
            let plant = land
                .into_iter()
                .find(|t| t.id == tile_id)
                .and_then(|t| t.plant)
                .ok_or_else(|| {
                    let e = "Couldn't find such a plant at this user's hackstead".to_string();
                    error!("{}", e);
                    e
                })?;
            let applicables: Vec<Possessed<Keepsake>> = inventory
                .into_iter()
                .filter_map(|x| x.try_into().ok())
                .filter(|x: &Possessed<Keepsake>| {
                    x.inner
                        .item_application
                        .as_ref()
                        .map(|item_appl| {
                            item_appl
                                .effects
                                .iter()
                                .any(|e| e.keep_plants.allows(&plant.name))
                        })
                        .unwrap_or(false)
                })
                .take(40)
                .collect();

            Modal {
                method: "open".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "item_apply_modal".to_string(),
                title: "Item + Plant = :D".to_string(),
                private_metadata: String::new(),
                blocks: vec![json!({
                    "type": "input",
                    "label": plain_text("Item Select"),
                    "block_id": "item_apply_input",
                    "element": {
                        "type": "static_select",
                        "placeholder": plain_text("Which item do ya wanna use on this plant?"),
                        "action_id": "item_apply_select",
                        // show them each applicable item they have sorted by category
                        "options": applicables
                            .into_iter()
                            .filter_map(|i| {
                                Some(json!({
                                    "text": plain_text(format!("{} {}", emojify(&i.name), i.name)),
                                    "description": plain_text(&i.inner.item_application.as_ref()?.short_description),
                                    // this is fucky-wucky because value can only be 75 chars
                                    "value": serde_json::to_string(&(
                                        &tile_id.to_simple().to_string(),
                                        i.id.to_simple().to_string(),
                                    )).unwrap(),
                                }))
                            })
                            .collect::<Vec<Value>>(),
                    }
                })],
                submit: Some("Apply!".to_string()),
            }
            .launch()
            .await?
        }
        "gotchi_hatch" => {
            info!("hatching egg!");

            to_farming
                .send(FarmingInputEvent::HatchEgg(
                    serde_json::from_str(&dbg!(action.value)).unwrap(),
                    i.user.id.clone(),
                ))
                .unwrap();

            json!({})
        }
        "gotchi_nickname" => {
            Modal {
                method: "push".to_string(),
                trigger_id: i.trigger_id,
                callback_id: "gotchi_nickname_modal".to_string(),
                title: "Nickname Gotchi".to_string(),
                private_metadata: i.view.ok_or("no view!".to_string())?.private_metadata,
                blocks: vec![json!({
                    "type": "input",
                    "block_id": "gotchi_nickname_block",
                    "label": plain_text("Nickname Gotchi"),
                    "element": {
                        "type": "plain_text_input",
                        "action_id": "gotchi_nickname_input",
                        "placeholder": plain_text("Nickname Gotchi"),
                        "initial_value": action.value,
                        "min_length": 1,
                        "max_length": 25,
                    }
                })],
                submit: Some("Change it!".to_string()),
                ..Default::default()
            }
            .launch()
            .await?
        }
        "possession_market_overview_page" => {
            let page_json = &action.value;
            let (item_name, cat): (String, Category) = serde_json::from_str(page_json).unwrap();

            let page = PossessionOverviewPage {
                credentials: if hacksteader::exists(&dyn_db(), i.user.id.clone()).await {
                    Credentials::Hacksteader
                } else {
                    Credentials::None
                },
                page: 0,
                interactivity: Interactivity::Buy,
                source: PossessionOverviewSource::Market(cat),
                item_name,
            };

            page.modal(i.trigger_id, "push").await?.launch().await?
        }
        "possession_page" => {
            let page_json = action.value;
            let mut page: PossessionPage = serde_json::from_str(&page_json).unwrap();

            if page.possession.steader == i.user.id {
                page.credentials = Credentials::Owner;
            }

            page.modal(i.trigger_id, "open").launch().await?
        }
        "push_possession_page" => {
            let page_json = action.value;
            let mut page: PossessionPage = serde_json::from_str(&page_json).unwrap();

            if page.possession.steader == i.user.id {
                page.credentials = Credentials::Owner;
            }

            page.modal(i.trigger_id, "push").launch().await?
        }
        "possession_overview_page" => {
            let page_json = action.value;
            let page: PossessionOverviewPage = serde_json::from_str(&page_json).unwrap();

            page.modal(i.trigger_id, "open").await?.launch().await?
        }
        "push_possession_overview_page" => {
            let page_json = action.value;
            let page: PossessionOverviewPage = serde_json::from_str(&page_json).unwrap();

            page.modal(i.trigger_id, "push").await?.launch().await?
        }
        _ => mrkdwn("huh?"),
    };

    Ok(ActionResponse::Json(Json(output_json)))
}

#[rocket::get("/steadercount")]
async fn steadercount() -> Result<String, String> {
    hcor::Profile::fetch_all(&dyn_db())
        .await
        .map(|profiles| profiles.len().to_string())
}

pub enum FarmingInputEvent {
    ActivateUser(String),
    RedeemLandCert(uuid::Uuid, String),
    HatchEgg(uuid::Uuid, String),
    ApplyItem(ItemApplication, String),
    PlantSeed(uuid::Uuid, hacksteader::Plant),
    BeginCraft {
        tile_id: uuid::Uuid,
        recipe_archetype_handle: usize,
    },
}

pub fn format_yield(items: Vec<Possession>, user: String) -> Vec<Value> {
    if items.len() < 8 {
        items
            .iter()
            .map(|p| {
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "*<@{}>'s new* {} _{}_!",
                        user,
                        emojify(&p.name),
                        p.name,
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!(
                            "http://{}/gotchi/img/{}/{}.png",
                            *URL,
                            format!(
                                "{:?}",
                                p.kind.category()
                            ).to_lowercase(),
                            filify(&p.name)
                        ),
                        "alt_text": "happy shiny give u stuffs",
                    }
                })
            })
            .collect::<Vec<_>>()
    } else {
        let mut occurrences: HashMap<_, usize> = Default::default();

        for p in &items {
            *occurrences
                .entry((
                    p.name.clone(),
                    format!("{:?}", p.kind.category()).to_lowercase(),
                ))
                .or_insert(0) += 1;
        }

        occurrences
            .iter()
            .map(|((name, category), count)| {
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "_<@{}>'s_ *{}* new {} _{}_!",
                        user,
                        count,
                        emojify(&name),
                        name
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!(
                            "http://{}/gotchi/img/{}/{}.png",
                            *URL,
                            category,
                            filify(&name)
                        ),
                        "alt_text": "happy shiny egg give u stuffs",
                    }
                })
            })
            .collect::<Vec<_>>()
    }
}

pub struct ItemApplication {
    tile: uuid::Uuid,
    item: uuid::Uuid,
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    use rocket_contrib::serve::StaticFiles;

    dotenv::dotenv().ok();
    pretty_env_logger::init();

    info!("starting");

    let (tx, rx) = crossbeam_channel::unbounded();

    tokio::task::spawn({
        use std::time::{Duration, SystemTime};
        use tokio::time::interval;

        let mut interval = interval(Duration::from_millis(FARM_CYCLE_MILLIS));

        let mut active_users: HashMap<String, bool> = HashMap::new();
        let mut plant_queue: HashMap<uuid::Uuid, hacksteader::Plant> = HashMap::new();
        // fix like this
        let mut craft_queue: HashMap<uuid::Uuid, config::ArchetypeHandle> = HashMap::new();
        let mut item_application_queue: HashMap<String, ItemApplication> = HashMap::new();
        let mut land_cert_queue: HashMap<String, uuid::Uuid> = HashMap::new();
        let mut hatch_egg_queue: HashMap<String, uuid::Uuid> = HashMap::new();

        async move {
            use futures::stream::{self, StreamExt, TryStreamExt};
            use hacksteader::{Plant, Tile};
            use hcor::Profile;

            loop {
                for (_, fresh) in active_users.iter_mut() {
                    *fresh = false;
                }

                while let Ok(farming_event) = rx.try_recv() {
                    use FarmingInputEvent::*;
                    match farming_event {
                        ActivateUser(name) => {
                            info!("activated: {}", name);
                            active_users.insert(name, true);
                        }
                        ApplyItem(application, user_id) => {
                            item_application_queue.insert(user_id, application);
                        }
                        PlantSeed(tile_id, plant) => {
                            plant_queue.insert(tile_id, plant);
                        }
                        RedeemLandCert(cert_id, user_id) => {
                            land_cert_queue.insert(user_id, cert_id);
                        }
                        HatchEgg(egg_id, user_id) => {
                            hatch_egg_queue.insert(user_id, egg_id);
                        }
                        BeginCraft {
                            tile_id,
                            recipe_archetype_handle,
                        } => {
                            craft_queue.insert(tile_id, recipe_archetype_handle);
                        }
                    }
                }

                interval.tick().await;
                info!("update!");

                if active_users.is_empty() {
                    info!("nobody on.");
                    continue;
                }

                let db = dyn_db();

                use rand::Rng;
                let mut deletions = vec![];
                let mut clear_plants = vec![];
                let mut possessions = vec![];
                let mut new_tiles = vec![];
                let mut dms: Vec<(String, Vec<Value>, String)> = Vec::new();
                let mut market_logs: Vec<(Vec<Value>, String)> = Vec::new();

                let mut hacksteaders: Vec<Hacksteader> = stream::iter(active_users.clone())
                    .map(|(id, _)| Hacksteader::from_db(&db, id))
                    .buffer_unordered(50)
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .filter_map(|hs| match hs {
                        Ok(i) => Some(i),
                        Err(e) => {
                            error!("error reading hacksteader from db: {}", e);
                            None
                        }
                    })
                    .collect();

                // Give away requested land/hatch eggs
                for hs in hacksteaders.iter_mut() {
                    if let Some((plant, appl, item)) =
                        item_application_queue.remove(&hs.user_id).and_then(|appl| {
                            let i = hs.inventory.iter().find(|i| i.id == appl.item)?;

                            Some((
                                hs.land
                                    .iter_mut()
                                    .find(|t| t.id == appl.tile)?
                                    .plant
                                    .as_mut()?,
                                i.kind.keepsake()?.item_application.as_ref()?,
                                i,
                            ))
                        })
                    {
                        deletions.push(Key::misc(item.id));
                        for (i, e) in appl.effects.iter().enumerate() {
                            match &e.kind {
                                config::ItemApplicationEffectKind::TurnsPlantInto(name) => {
                                    plant.archetype_handle =
                                        CONFIG.find_plant_handle(name).expect("invalid handle");

                                    for (i, e) in plant
                                        .effects
                                        .clone()
                                        .iter()
                                        .filter_map(|e| {
                                            CONFIG.get_item_application_effect(
                                                e.item_archetype_handle,
                                                e.effect_archetype_handle,
                                            )
                                        })
                                        .enumerate()
                                    {
                                        if !e
                                            .keep_plants
                                            .lookup_handles()
                                            .unwrap()
                                            .allows(&plant.archetype_handle)
                                        {
                                            plant.effects.swap_remove(i);
                                        }
                                    }
                                }
                                _ => {}
                            }

                            plant.effects.push(hacksteader::Effect {
                                until_finish: e.duration,
                                item_archetype_handle: item.archetype_handle,
                                effect_archetype_handle: i,
                            });
                        }
                    }
                    if let Some(cert_id) = land_cert_queue.remove(&hs.user_id) {
                        if hs.inventory.iter().any(|p| {
                            let same_id = p.id == cert_id;
                            let actually_land_cert = p
                                .kind
                                .keepsake()
                                .filter(|k| k.unlocks_land.is_some())
                                .is_some();

                            same_id && actually_land_cert
                        }) {
                            deletions.push(Key::misc(cert_id));
                            let new_tile = hacksteader::Tile::new(hs.user_id.clone());
                            hs.land.push(new_tile.clone());
                            new_tiles.push(new_tile.clone());
                        }
                    }
                    if let Some(egg_id) = hatch_egg_queue.remove(&hs.user_id) {
                        info!("egg hatch requested!");

                        if let Some((p, hatch_table)) = hs.gotchis.iter().find_map(|g| {
                            Some(g).filter(|g| g.id == egg_id).and_then(|g| {
                                info!("hatching {:?}", g);
                                Some((g, g.inner.hatch_table.as_ref()?))
                            })
                        }) {
                            deletions.push(Key::gotchi(egg_id));

                            let (spawn_handles, percentile) =
                                config::spawn_with_percentile(hatch_table, &mut rand::thread_rng());

                            let spawned: Vec<Possession> = spawn_handles
                                .into_iter()
                                .map(|h| {
                                    Possession::new(
                                        CONFIG.find_possession_handle(&h).unwrap(),
                                        possess::Owner::hatcher(hs.user_id.clone()),
                                    )
                                })
                                .collect();

                            let mut msg = vec![
                                json!({
                                    "type": "section",
                                    "text": mrkdwn(format!(
                                        concat!(
                                            "*<@{}> hatched a {}!*\n",
                                            "The rarity of this loot puts it in the ",
                                            "*{:.2}th* percentile for loot from eggs of this type.",
                                        ),
                                        hs.user_id,
                                        p.name,
                                        percentile * 100.0
                                    )),
                                    "accessory": {
                                        "type": "image",
                                        "image_url": format!(
                                            "http://{}/gotchi/img/{}/{}.png",
                                            *URL,
                                            format!(
                                                "{:?}",
                                                p.kind.category()
                                            ).to_lowercase(),
                                            filify(&p.name)
                                        ),
                                        "alt_text": "happy shiny egg give u stuffs",
                                    }
                                }),
                                comment("WAT I THOUGHT IT WAS ROCK"),
                                json!({ "type": "divider" }),
                            ];

                            possessions.extend_from_slice(&spawned);

                            msg.append(&mut format_yield(spawned, hs.user_id.clone()));
                            dms.push((
                                hs.user_id.clone(),
                                msg.clone(),
                                format!("Your {} hatched!", p.name),
                            ));
                            market_logs
                                .push((msg, format!("<@{}> hatched a {}!", hs.user_id, p.name)));
                        } else {
                            warn!("egg hatch ignored; hack attempt?")
                        }
                    }
                }

                // Launch requested crafts
                for hs in hacksteaders.iter_mut() {
                    let nb = hs.neighbor_bonuses();
                    let Hacksteader {
                        inventory, land, ..
                    } = hs;

                    let mut land_iter = land.iter_mut();
                    while let Some(Tile {
                        plant: Some(ref mut plant),
                        steader,
                        id,
                        ..
                    }) = land_iter.next()
                    {
                        let config::PlantAdvancementSum {
                            recipes,
                            craft_return_chance,
                            ..
                        } = plant.advancements_sum(
                            nb.clone()
                                .bonuses_for_plant(*id, plant.archetype_handle)
                                .iter(),
                        );

                        if let Some((recipe_archetype_handle, recipe)) = craft_queue
                            .remove(&id)
                            .filter(|_| plant.craft.is_none())
                            .and_then(|i| Some((i, recipes.get(i)?)))
                        {
                            let should_take: usize =
                                recipe.needs.iter().map(|(n, _)| n).sum::<usize>();
                            let used_resources = recipe
                                .needs
                                .clone()
                                .into_iter()
                                .flat_map(|(count, ah)| {
                                    inventory
                                        .iter()
                                        .filter(move |p| p.archetype_handle == ah)
                                        .take(count)
                                })
                                .collect::<Vec<_>>();

                            if should_take == used_resources.len() {
                                let mut rng = rand::thread_rng();
                                deletions.append(
                                    &mut used_resources
                                        .into_iter()
                                        .filter(|p| {
                                            let keep = rng.gen_range(0.0, 1.0) < craft_return_chance;
                                            if keep {
                                                info!("mommy can we keep it? YES? YESSS");
                                                dms.push((
                                                    steader.clone(),
                                                    vec![
                                                        comment("your craft return bonus just came in quite handy!"),
                                                        comment(format!(
                                                            "looks like you get to keep a {} from that craft!",
                                                            &p.name,
                                                        )),
                                                    ],
                                                    "What's this, a crafting bonus‽".to_string()
                                                ));
                                            }
                                            !keep
                                        })
                                        .map(|p| p.key())
                                        .collect()
                                );

                                info!("submitting craft");
                                plant.craft = Some(hacksteader::Craft {
                                    until_finish: recipe.time,
                                    recipe_archetype_handle,
                                });
                            } else {
                                dms.push((
                                    steader.clone(),
                                    vec![
                                        comment("you don't have enough resources to craft that"),
                                        comment("nice try tho"),
                                    ],
                                    "You sure you have enough to craft that? Check again..."
                                        .to_string(),
                                ));
                            }
                        }
                    }
                }

                // we'll be frequently looking up profiles by who owns them to award xp.
                let mut profiles: HashMap<String, Profile> = hacksteaders
                    .iter()
                    .map(|hs| (hs.user_id.clone(), hs.profile.clone()))
                    .collect();

                // we only want to update the time on someone's profile once
                // even though they might have several plants, any of which
                // might be boosted, so we give them a "plant token" for
                // each of their plants, and move them forward when they
                // run out of tokens
                let mut plant_tokens: HashMap<String, usize> = hacksteaders
                    .iter()
                    .map(|hs| {
                        (
                            hs.user_id.clone(),
                            hs.land.iter().filter_map(|t| t.plant.as_ref()).count(),
                        )
                    })
                    .collect();

                // same goes with the neighbor bonuses for each hackstead
                let neighbor_bonuses: HashMap<String, _> = hacksteaders
                    .iter()
                    .map(|hs| (hs.user_id.clone(), hs.neighbor_bonuses()))
                    .collect();

                // we can only farm on tiles with plants,
                let mut tiles: Vec<(Plant, Tile)> = hacksteaders
                    .into_iter()
                    .flat_map(|hs| hs.land.into_iter())
                    .filter_map(|mut t| {
                        Some((
                            t.plant.take().or_else(|| {
                                plant_queue.remove(&t.id).map(|plant| {
                                    profiles
                                        .get_mut(&t.steader)
                                        .expect("tile has no owner")
                                        .last_farm = SystemTime::now();
                                    plant
                                })
                            })?,
                            t,
                        ))
                    })
                    .collect();

                // remove inactive users
                for ((_, profile), (user, fresh)) in
                    profiles.iter_mut().zip(active_users.clone().into_iter())
                {
                    let now = std::time::SystemTime::now();
                    if fresh {
                        profile.last_active = now;
                    } else {
                        const ACTIVE_DURATION_SECS: u64 = 60 * 5;
                        if now
                            .duration_since(profile.last_active)
                            .ok()
                            .filter(|r| r.as_secs() >= ACTIVE_DURATION_SECS)
                            .is_some()
                        {
                            active_users.remove(&user);
                        }
                    }
                }

                // game tick loop:
                // this is where we go through and we increment each xp/craft/yield
                for (plant, tile) in tiles.iter_mut() {
                    let profile = match profiles.get_mut(&tile.steader) {
                        Some(profile) => profile,
                        None => {
                            error!(
                                concat!(
                                    "ignoring 1 active user: ",
                                    "couldn't get tile[{}]'s steader[{}]'s profile",
                                ),
                                tile.id, tile.steader
                            );
                            continue;
                        }
                    };

                    let neighbor_bonuses = match neighbor_bonuses.get(&tile.steader) {
                        Some(bonuses) => bonuses,
                        None => {
                            error!(
                                concat!(
                                    "ignoring 1 active user: ",
                                    "couldn't get tile[{}]'s steader[{}]'s neighbor bonuses",
                                ),
                                tile.id, tile.steader
                            );
                            continue;
                        }
                    };
                    let neighbor_bonuses = neighbor_bonuses
                        .clone()
                        .bonuses_for_plant(tile.id, plant.archetype_handle);

                    // amount of farm cycles since the last farm, rounded down
                    let elapsed = SystemTime::now()
                        .duration_since(profile.last_farm)
                        .unwrap_or_default()
                        .as_millis()
                        / (FARM_CYCLE_MILLIS as u128);

                    // increment their profile's "last farm" time so we can calculate
                    // an accurate "elapsed" during the next update.
                    if elapsed > 0 {
                        if let Some(tokens) = plant_tokens.get_mut(&profile.id) {
                            *tokens = *tokens - 1;
                            if *tokens == 0 {
                                info!("all plants finished for {}", profile.id);
                                // we don't want to add the boosted_elapsed here, then your item effects
                                // would have to be "paid for" later (your farm wouldn't work for however
                                // much time the effect gave you).
                                profile.last_farm += Duration::from_millis(
                                    (FARM_CYCLE_MILLIS as u128 * elapsed)
                                        .try_into()
                                        .unwrap_or_else(|e| {
                                            error!(
                                                "too many farm cycle millis * elapsed[{}]: {}",
                                                elapsed, e
                                            );
                                            0
                                        }),
                                );
                            }
                        }
                    }

                    info!("elapsing {} cycles for {}", elapsed, profile.id);

                    for _ in 0..elapsed {
                        plant.effects = plant
                            .effects
                            .iter_mut()
                            .filter_map(|e| {
                                if let Some(uf) = e.until_finish.as_mut() {
                                    // decrement counter, remove if 0
                                    *uf = (*uf - 1.0).max(0.0);
                                    if *uf == 0.0 {
                                        info!("removing effect: {:?}", e);
                                        return None;
                                    }
                                }

                                Some(*e)
                            })
                            .collect::<Vec<_>>();

                        // you want to recalculate this every update because it's dependent
                        // on what effects are active, especially the `total_extra_time_ticks`.
                        let plant_sum = plant.advancements_sum(neighbor_bonuses.iter());

                        let ticks = plant_sum.total_extra_time_ticks + 1;
                        let mut rng = rand::thread_rng();
                        info!("triggering {} ticks for {}'s cycle", ticks, profile.id);
                        for _ in 0..ticks {
                            plant.craft = match plant
                                .current_recipe_raw()
                                .and_then(|r| Some((r, plant.craft.take()?)))
                            {
                                Some((recipe, mut craft)) => {
                                    if craft.until_finish > plant_sum.crafting_speed_multiplier {
                                        craft.until_finish -= plant_sum.crafting_speed_multiplier;
                                        Some(craft)
                                    } else {
                                        let earned_xp = {
                                            let (lo, hi) = recipe.xp;
                                            rng.gen_range(lo, hi)
                                        };
                                        plant.queued_xp_bonus += earned_xp;

                                        let mut output: Vec<Possession> = recipe
                                            .makes
                                            .clone()
                                            .output()
                                            .into_iter()
                                            .map(|ah| {
                                                Possession::new(
                                                    ah,
                                                    possess::Owner::crafter(tile.steader.clone()),
                                                )
                                            })
                                            .collect();

                                        if rng.gen_range(0.0, 1.0)
                                            < plant_sum.double_craft_yield_chance
                                        {
                                            info!("cloning recipe output! {:?}", output);
                                            output.append(&mut output.clone());
                                            info!("after clone: {:?}", output);
                                        }
                                        possessions.extend_from_slice(&output);

                                        let mut msg = vec![
                                            json!({
                                                "type": "section",
                                                "text": mrkdwn(format!(
                                                    concat!(
                                                        "Your *{}* has finished crafting *{}* for you!\n",
                                                        "This earned it {} xp!",
                                                    ),
                                                    plant.name,
                                                    recipe.clone().lookup_handles().unwrap().title(),
                                                    earned_xp,
                                                )),
                                                "accessory": {
                                                    "type": "image",
                                                    "image_url": format!(
                                                        "http://{}/gotchi/img/plant/{}.gif",
                                                        *URL,
                                                        filify(&plant.current_advancement().art)
                                                    ),
                                                    "alt_text": "happy shiny plant give u stuffs",
                                                }
                                            }),
                                            comment("YAY FREE STUFFZ 'CEPT LIKE IT'S NOT FREE"),
                                            json!({ "type": "divider" }),
                                        ];

                                        msg.append(&mut format_yield(output, tile.steader.clone()));
                                        dms.push((
                                            tile.steader.clone(),
                                            msg,
                                            format!(
                                                "What's this, a new {}?",
                                                recipe.clone().lookup_handles().unwrap().title()
                                            ),
                                        ));

                                        if recipe.destroys_plant {
                                            clear_plants.push(tile.id.clone());
                                        }

                                        None
                                    }
                                }
                                None => None,
                            };

                            plant.until_yield = match plant.until_yield
                                - plant_sum.yield_speed_multiplier
                            {
                                n if n > 0.0 => n,
                                _ if plant.base_yield_duration.is_some() => {
                                    let owner = &tile.steader;
                                    let (yielded, xp_bonuses): (Vec<_>, Vec<_>) =
                                        config::spawn(&plant_sum.yields, &mut rand::thread_rng())
                                            .map(|(ah, xp)| {
                                                (
                                                    Possession::new(
                                                        ah,
                                                        possess::Owner::farmer(owner.clone()),
                                                    ),
                                                    xp,
                                                )
                                            })
                                            .unzip();
                                    let earned_xp = xp_bonuses.into_iter().sum::<usize>() as u64;

                                    plant.queued_xp_bonus += earned_xp;

                                    let mut msg = vec![
                                        json!({
                                            "type": "section",
                                            "text": mrkdwn(format!(
                                                concat!(
                                                    "Your *{}* has produced a crop yield for you!\n",
                                                    "This earned it {} xp!"
                                                ),
                                                plant.name,
                                                earned_xp,
                                            )),
                                            "accessory": {
                                                "type": "image",
                                                "image_url": format!(
                                                    "http://{}/gotchi/img/plant/{}.gif",
                                                    *URL,
                                                    filify(&plant.current_advancement().art)
                                                ),
                                                "alt_text": "happy shiny plant give u stuffs",
                                            }
                                        }),
                                        comment("FREE STUFF FROM CUTE THING"),
                                        json!({ "type": "divider" }),
                                    ];

                                    possessions.extend_from_slice(&yielded);

                                    msg.append(&mut format_yield(yielded, tile.steader.clone()));

                                    dms.push((
                                        tile.steader.clone(),
                                        msg,
                                        "FREE STUFF FROM CUTE THING".to_string(),
                                    ));

                                    plant.base_yield_duration.unwrap_or(0.0)
                                }
                                n => n,
                            };

                            if let Some(advancement) = plant.increase_xp(plant_sum.xp_multiplier) {
                                dms.push((tile.steader.clone(), vec![
                                    json!({
                                        "type": "section",
                                        "text": mrkdwn(format!(
                                            concat!(
                                                ":tada: Your _{}_ is now a *{}*!\n\n",
                                                "*{}* Achieved:\n _{}_\n\n",
                                                ":stonks: Total XP: *{}xp*",
                                            ),
                                            plant.name,
                                            advancement.achiever_title,
                                            advancement.title,
                                            advancement.description,
                                            advancement.xp
                                        )),
                                        "accessory": {
                                            "type": "image",
                                            "image_url": format!("http://{}/gotchi/img/plant/{}.gif", *URL, filify(&advancement.art)),
                                            "alt_text": "happy shiny better plant",
                                        }
                                    }),
                                    comment("EXCITING LEVELING UP NOISES"),
                                ],
                                format!(
                                    "Your {} is now a {}!",
                                    plant.name,
                                    advancement.achiever_title
                                )
                            ))
                            }
                            let profile_sum =
                                profile.advancements.sum(profile.xp, std::iter::empty());
                            if let Some(advancement) = profile.increase_xp(plant_sum.xp_multiplier)
                            {
                                dms.push((tile.steader.clone(), vec![
                                    json!({
                                        "type": "section",
                                        "text": mrkdwn(format!(
                                            concat!(
                                                ":tada: Your _Hackstead_ is now a *{}*!\n\n",
                                                "*{}* Achieved:\n_{}_\n\n",
                                                ":stonks: Total XP: *{}xp*\n",
                                                ":mountain: Land Available: *{} pieces* _(+{} pieces)_"
                                            ),
                                            advancement.achiever_title,
                                            advancement.title,
                                            advancement.description,
                                            advancement.xp,
                                            profile_sum.land,
                                            match advancement.kind {
                                                config::HacksteadAdvancementKind::Land { pieces } => pieces,
                                            }
                                        )),
                                        "accessory": {
                                            "type": "image",
                                            "image_url": format!("http://{}/gotchi/img/icon/seedlet.png", *URL),
                                            "alt_text": "happy shiny better hackstead",
                                        }
                                    }),
                                    comment("SUPER EXCITING LEVELING UP NOISES"),
                                ],
                                format!("Your Hackstead is now a {}!", advancement.achiever_title)
                            ));
                            }
                        }
                    }
                }

                let _ = stream::iter(
                    tiles
                        .into_iter()
                        .map(|(plant, mut tile)| rusoto_dynamodb::WriteRequest {
                            put_request: Some(rusoto_dynamodb::PutRequest {
                                item: {
                                    tile.plant = if clear_plants.iter().any(|id| *id == tile.id) {
                                        None
                                    } else {
                                        Some(plant)
                                    };

                                    tile.into_av().m.expect("tile attribute should be map")
                                },
                            }),
                            ..Default::default()
                        })
                        .chain(
                            new_tiles
                                .into_iter()
                                .map(|t| rusoto_dynamodb::WriteRequest {
                                    put_request: Some(rusoto_dynamodb::PutRequest {
                                        item: t.into_av().m.expect("tile attribute should be map"),
                                    }),
                                    ..Default::default()
                                }),
                        )
                        .chain(profiles.iter().map(|(_, p)| rusoto_dynamodb::WriteRequest {
                            put_request: Some(rusoto_dynamodb::PutRequest { item: p.item() }),
                            ..Default::default()
                        }))
                        .chain(possessions.iter().map(|p| rusoto_dynamodb::WriteRequest {
                            put_request: Some(rusoto_dynamodb::PutRequest { item: p.item() }),
                            ..Default::default()
                        }))
                        .chain(
                            deletions
                                .into_iter()
                                .map(|key| rusoto_dynamodb::WriteRequest {
                                    delete_request: Some(rusoto_dynamodb::DeleteRequest {
                                        key: key.into_item(),
                                    }),
                                    ..Default::default()
                                }),
                        )
                        .collect::<Vec<_>>()
                        .chunks(25)
                        .map(|items| {
                            db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
                                request_items: [(hcor::TABLE_NAME.to_string(), items.to_vec())]
                                    .iter()
                                    .cloned()
                                    .collect(),
                                ..Default::default()
                            })
                        }),
                )
                .map(|x| Ok(x))
                .try_for_each_concurrent(None, |r| async move {
                    match r.await {
                        Ok(_) => Ok(()),
                        Err(e) => Err(format!("error updating db after farm cycle: {}", e)),
                    }
                })
                .await
                .map_err(|e| error!("farm cycle async err: {}", e));

                let _ = futures::try_join!(
                    stream::iter(profiles.clone())
                        .map(|x| Ok(x))
                        .try_for_each_concurrent(None, |(who, _)| { update_user_home_tab(who) }),
                    stream::iter(dms).map(|x| Ok(x)).try_for_each_concurrent(
                        None,
                        |(who, blocks, craft_type)| {
                            dm_blocks(who.clone(), craft_type.clone(), blocks.to_vec())
                        }
                    ),
                    stream::iter(market_logs)
                        .map(|x| Ok(x))
                        .try_for_each_concurrent(None, |(blocks, notif_type)| {
                            market::log_blocks(notif_type, blocks)
                        }),
                )
                .map_err(|e| error!("farm cycle async err: {}", e));
            }
        }
    });

    rocket::ignite()
        .manage(tx)
        .mount(
            "/gotchi",
            routes![
                hackstead,
                hackmarket,
                action_endpoint,
                egghatchwhen,
                hgive,
                event::challenge,
                event::non_challenge_event,
                stateofsteading,
                steadercount
            ],
        )
        .mount(
            "/gotchi/img",
            StaticFiles::from(concat!(env!("CARGO_MANIFEST_DIR"), "/img")),
        )
        .launch()
        .await
        .expect("launch fail");

    Ok(())
}
