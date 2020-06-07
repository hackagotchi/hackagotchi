#![feature(decl_macro)]
#![feature(proc_macro_hygiene)]
#![feature(try_trait)]
#![recursion_limit = "512"]
use config::CONFIG;
use core::config;
use core::frontend::emojify;
use core::possess;
use core::{Category, Key};
use crossbeam_channel::Sender;
use log::*;
use possess::{Possessed, Possession};
use regex::Regex;
use rocket::request::LenientForm;
use rocket::tokio;
use rocket::{post, routes, FromForm, State};
use rocket_contrib::json::Json;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};
use serde_json::{json, Value};
use std::convert::TryInto;

pub mod banker;
pub mod event;
pub mod hacksteader;
pub mod market;
mod yank_config;

use hacksteader::Hacksteader;

pub fn dyn_db() -> DynamoDbClient {
    DynamoDbClient::new_with(
        {
            let mut c = rusoto_core::HttpClient::new().unwrap();
            c.local_agent("http://localhost:8000".to_string());
            c
        },
        rusoto_credential::EnvironmentProvider::default(),
        rusoto_core::Region::UsEast1
    )
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

pub async fn dm_blocks(user_id: String, blocks: Vec<Value>) -> Result<(), String> {
    let o = json!({
        "channel": user_id,
        "token": *TOKEN,
        "blocks": blocks
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

    async fn modal(self, trigger_id: String, method: &'static str) -> Result<Modal, String> {
        Ok(Modal {
            callback_id: self.callback_id(),
            blocks: self.blocks().await?,
            submit: None,
            title: self.item_name,
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
                        if let Some(core::market::Sale { price, .. }) = possession.sale {
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
    fn modal(self, trigger_id: String, method: &'static str) -> Modal {
        Modal {
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            submit: self.submit(),
            title: self.possession.nickname().to_string(),
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
            title: self.possession.nickname().to_string(),
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

        if self.possession.kind.is_gotchi() {
            blocks.push(actions(
                "gotchi",
                &[("Nickname", Some(json!(possession.nickname())))],
            ));
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
        "If you had enough Land Deeds, you could buy {} pieces of land",
        hs_adv_sum.land
    )));

    blocks.push(json!({ "type": "divider" }));
    let tiles_owned = land.len();
    for tile in land.into_iter() {
        if let Some(p) = tile.plant.as_ref() {
            let neighbor_bonuses = neighbor_bonuses
                .clone()
                .bonuses_for_plant(
                    tile.id,
                    p.archetype_handle
                );
            let sum = p.advancements.sum(p.xp, neighbor_bonuses.iter());
            let unboosted_sum = p.advancements.raw_sum(p.xp);
            let ca = p.current_advancement();

            blocks.push(json!({
                "type": "section",
                "text": mrkdwn({
                    let mut s = String::new();
                    s.push_str(&format!(
                        "*{}* - _{}_ - *{}lvl* - {}xp\n\n",
                        p.name,
                        ca.achiever_title,
                        p.advancements.current_position(p.xp),
                        p.xp
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
            if !sum.yields.is_empty() {
                blocks.push(json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "*Yield*\n{}  {:.3} minutes to go",
                        progress_bar(30, 1.0 - p.until_yield/p.base_yield_duration),
                        (p.until_yield / sum.yield_speed_multiplier) / FARM_CYCLES_PER_MIN as f32
                    )),
                    "accessory": {
                        "type": "button",
                        "text": plain_text("Yield Stats"),
                        "value": serde_json::to_string(&(user_id.to_string(), tile.id)).unwrap(),
                        "action_id": "yield_stats",
                    }
                }));
            }
            if !sum.recipes.is_empty() {
                let recipes = sum.recipes.iter().map(|r| (r.satisfies(&inventory), r));
                if let Some(craft) = &p.craft {
                    blocks.push(json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "*Crafting {}*\n{}  {:.3} minutes to go",
                            CONFIG
                                .possession_archetypes
                                .get(craft.makes)
                                .map(|x| x.name.as_str())
                                .unwrap_or("unknown"),
                            progress_bar(30, 1.0 - craft.until_finish/craft.total_cycles),
                            (craft.until_finish / sum.yield_speed_multiplier) / FARM_CYCLES_PER_MIN as f32
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
                            "value": serde_json::to_string(&(
                                &tile.id,
                                recipes.collect::<Vec<_>>()
                            )).unwrap(),
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

                let applicables: Vec<String> = inventory
                    .iter()
                    .cloned()
                    .filter_map(|x| {
                        x.kind.keepsake()?.item_application_effect.as_ref()?;
                        Some(x.id.to_simple().to_string())
                    })
                    .collect();

                if !applicables.is_empty() && interactivity.write() {
                    actions.push(json!({
                        "type": "button",
                        "text": plain_text("Apply Item"),
                        "style": "primary",
                        "value": serde_json::to_string(&(tile.id.to_simple().to_string(), applicables)).unwrap(),
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

    blocks.push(json!({ "type": "divider" }));

    if inventory.len() > 0 {
        blocks.push(json!({
            "type": "section",
            "text": mrkdwn("*Inventory*"),
        }));

        let mut occurences: std::collections::HashMap<String, Vec<Possession>> = Default::default();

        for possession in inventory.into_iter() {
            occurences
                .entry(possession.name.clone())
                .or_insert(vec![])
                .push(possession)
        }

        let mut inv_entries = occurences.into_iter().collect::<Vec<_>>();
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
                        "action_id": "possession_page",
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
                        "action_id": "possession_overview_page",
                    }
                }));
            }
        }
    } else {
        blocks.push(comment("Your inventory is empty"));
    }

    if gotchis.len() > 0 {
        blocks.push(json!({ "type": "divider" }));

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(match gotchis.len() {
                1 => "*Your Hackagotchi*".into(),
                _ => format!("*Your {} Hackagotchi*", gotchis.len())
            }),
        }));

        let total_happiness = gotchis.iter().map(|g| g.inner.base_happiness).sum::<u64>();

        blocks.push(json!({
            "type": "actions",
            "elements": [{
                "type": "button",
                "text": plain_text("See Gotchi"),
                "style": "primary",
                "value": serde_json::to_string(&(&user_id, interactivity, credentials, false)).unwrap(),
                "action_id": "gotchi_overview",
            }],
        }));
        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(format!("Total happiness: *{}*", total_happiness))
        }));
        blocks.push(comment(
            "The total happiness of all your gotchi is equivalent to the \
             amount of GP you'll get at the next Harvest.",
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

macro_rules! hacksteader_opening_blurb { ( $hackstead_cost:expr ) => { format!(
"
*Your Own Hackagotchi Hackstead!*

:corn: Grow your own Farmables which make Hackagotchi happier!
:sparkling_heart: Earn passive income by collecting adorable Hackagotchi!
:money_with_wings: Buy, sell and trade Farmables and Hackagotchi at an open market!

Hacksteading costs *{} GP*.
As a Hacksteader, you'll have a plot of land on which to grow your own Farmables which make Hackagotchi happier. \
Happier Hackagotchi generate more passive income! \
You can also buy, sell, and trade Farmables and Hackagotchi for GP on an open market space. \
",
$hackstead_cost
) } }

fn hackstead_explanation_blocks() -> Vec<Value> {
    vec![
        json!({
            "type": "section",
            "text": mrkdwn(hacksteader_opening_blurb!(*HACKSTEAD_PRICE)),
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
                    "title": plain_text("Do you have what it takes"),
                    "text": mrkdwn("to be a Hackagotchi Hacksteader?"),
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
    use std::collections::HashMap;

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
            .take(entry_count * 2 - 1),
    )
    .collect()
}

#[derive(FromForm, Debug)]
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
    use std::collections::HashMap;

    let profiles = core::Profile::fetch_all(&dyn_db()).await.unwrap();
    let tiles = hacksteader::Tile::fetch_all(&dyn_db()).await.unwrap();

    struct PlantEntry {
        owner: String,
        seed_from: String,
        level: usize,
    }

    let mut archetype_occurences: HashMap<config::PlantArchetype, Vec<PlantEntry>> =
        Default::default();

    for tile in tiles.iter() {
        let plant = match &tile.plant {
            Some(plant) => plant,
            None => continue,
        };

        archetype_occurences
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
            archetype_occurences
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
        archetype_occurences
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
                    table_name: core::TABLE_NAME.to_string(),
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
                .await
                .map_err(|e| e)?;

                // update the home tab
                // TODO: make this not read from the database
                update_user_home_tab(user.id.clone()).await?;

                let possession = hacksteader::get_possession(&dyn_db(), key).await?;

                // DM the new_owner about their new acquisition!
                dm_blocks(new_owner.clone(), {
                    // TODO: with_capacity optimization
                    let mut blocks = vec![
                        json!({
                            "type": "section",
                            "text": mrkdwn(format!(
                                "<@{}> has been so kind as to gift you a {} _{}_!",
                                user.id,
                                emojify(&possession.name),
                                possession.nickname()
                            ))
                        }),
                        json!({ "type": "divider" }),
                    ];
                    let page = PossessionPage {
                        interactivity: Interactivity::Read,
                        credentials: Credentials::Owner,
                        possession,
                    };
                    blocks.append(&mut page.blocks());
                    blocks.push(json!({ "type": "divider" }));
                    blocks.push(comment(format!(
                        "Manage all of your possessions like this one at your <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>",
                        *APP_ID,
                    )));
                    blocks
                }).await?;

                // close ALL THE MODALS!!!
                return Ok(ActionResponse::Json(Json(json!({
                    "response_action": "clear",
                }))));
            }
        } else if let Some((view, None, _trigger_id, values, user)) = view {
            debug!("view state values: {:#?}", values);

            match view.callback_id.as_str() {
                "crafting_confirm_modal" => {
                    let (tile_id, recipe): (uuid::Uuid, config::Recipe<config::ArchetypeHandle>) =
                        serde_json::from_str(&view.private_metadata).unwrap();

                    to_farming
                        .send(FarmingInputEvent::BeginCraft { tile_id, recipe })
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
            if let Some((tile_id, item_id, item_ah)) = values
                .get("item_apply_input")
                .and_then(|i| i.get("item_apply_select"))
                .and_then(|s| s.get("selected_option"))
                .and_then(|s| s.get("value"))
                .and_then(|s| s.as_str())
                .and_then(|v| serde_json::from_str(v).ok())
            {
                info!("applying item!");
                let db = dyn_db();
                Hacksteader::delete(&db, Key::misc(item_id))
                    .await
                    .map_err(|e| {
                        let a = format!("couldn't remove item after applying: {}", e);
                        error!("{}", a);
                        a
                    })?;

                to_farming
                    .send(FarmingInputEvent::ApplyItem(tile_id, item_ah))
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
                .take(40)
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
                                let options = seeds
                                    .iter()
                                    .filter(|s| s.inner.grows_into == pa.name)
                                    .map(|s| {
                                        json!({
                                            "text": plain_text(format!("{} {}", emojify(&s.name), s.name)),
                                            "description": plain_text(format!(
                                                "{} generations old", 
                                                s.inner.pedigree.iter().map(|sg| sg.generations).sum::<u64>()
                                            )),
                                            // this is fucky-wucky because value can only be 75 chars
                                            "value": serde_json::to_string(&(
                                                &tile_id.to_simple().to_string(),
                                                s.id.to_simple().to_string(),
                                            )).unwrap(),
                                        })
                                    })
                                    .collect::<Vec<Value>>();

                                if options.is_empty() {
                                    None
                                } else {
                                    Some(json!({
                                        "label": plain_text(&pa.name),
                                        "options": options,
                                    }))
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
            let (steader, interactivity, credentials, push):
                (String, Interactivity, Credentials, bool) = serde_json::from_str(&action.value).unwrap();

            let hs = Hacksteader::from_db(&dyn_db(), steader).await?;

            let blocks = hs.gotchis.into_iter().map(|gotchi| {
                json!({
                    "type": "section",
                    "text": mrkdwn(format!("_{} ({}, {} happiness)_", emojify(&gotchi.name), gotchi.name, gotchi.inner.base_happiness)),
                    "accessory": {
                        "type": "button",
                        "style": "primary",
                        "text": plain_text(&gotchi.inner.nickname),
                        "value": serde_json::to_string(&PossessionPage {
                            possession: gotchi.into_possession(),
                            interactivity,
                            credentials,
                        }).unwrap(),
                        "action_id": "push_possession_page",
                    }
                })
            })
            .collect();

            Modal {
                method: if push { "push" } else { "open" }.to_string(),
                trigger_id: i.trigger_id,
                callback_id: "crafting_confirm_modal".to_string(),
                title: "Crafting Confirmation".to_string(),
                private_metadata: action.value.to_string(),
                blocks,
                submit: None,
            }
            .launch()
            .await?
        }
        "crafting_confirm" => {
            let craft_json = &action.value;
            let (plant_id, raw_recipe): (uuid::Uuid, config::Recipe<config::ArchetypeHandle>) =
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

            let neighbor_bonuses = all_nb
                .bonuses_for_plant(
                    plant_id,
                    plant.archetype_handle
                );

            let sum = plant.advancements.sum(plant.xp, neighbor_bonuses.iter());

            let recipe = raw_recipe
                .lookup_handles()
                .ok_or_else(|| "invalid recipe".to_string())?;
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
                            "using\n{}\n{}",
                        ),
                        (recipe.time / sum.yield_speed_multiplier) / FARM_CYCLES_PER_MIN as f32,
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
                        "image_url": format!("http://{}/gotchi/img/{}/{}.png",
                            *URL,
                            possible_output.kind.category(),
                            filify(&possible_output.name)
                        ),
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

            let (tile_id, recipes): (
                uuid::Uuid,
                Vec<(bool, config::Recipe<config::ArchetypeHandle>)>,
            ) = serde_json::from_str(&action.value).unwrap();
            let tile = hacksteader::get_tile(&dyn_db(), tile_id)
                .await
                .map_err(|e| {
                    let a = format!("couldn't get tile to craft: {}", e);
                    error!("{}", a);
                    a
                })?;

            let blocks = recipes
                .into_iter()
                .flat_map(|(possible, raw_recipe)| {
                    let recipe = raw_recipe
                        .clone()
                        .lookup_handles()
                        .expect("invalid archetype handle");
                    let mut b = Vec::with_capacity(recipe.needs.len() + 2);
                    let possible_output = recipe.makes.any();

                    let mut head = json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "{} *{}*\n_{}_",
                            emojify(&possible_output.name),
                            possible_output.name,
                            possible_output.description
                        )),
                    });
                    if possible && tile.steader == i.user.id {
                        head.as_object_mut().unwrap().insert(
                            "accessory".to_string(),
                            json!({
                                "type": "button",
                                "style": "primary",
                                "text": plain_text(format!("Craft {}", possible_output.name)),
                                "value": serde_json::to_string(&(
                                    &tile_id,
                                    raw_recipe,
                                )).unwrap(),
                                "action_id": "crafting_confirm",
                            }),
                        );
                    }
                    b.push(head);

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
            let neighbor_bonuses = all_nb
                .bonuses_for_plant(
                    plant_id,
                    plant.archetype_handle
                );

            let advancements = plant
                .advancements
                .unlocked(plant.xp)
                .filter(|a| match &a.kind {
                    Neighbor(..) => false,
                    _ => true,
                })
                .chain(neighbor_bonuses.iter())
                .collect::<Vec<_>>();
            let sum = plant
                .advancements
                .sum(plant.xp, neighbor_bonuses.iter());
            let yield_farm_cycles = plant.base_yield_duration / sum.yield_speed_multiplier;

            let mut blocks = vec![];

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
            for adv in advancements.iter() {
                match &adv.kind {
                    YieldSpeed(s) => {
                        blocks.push(comment(format!("_{}_: *x{}* speed boost", adv.title, s)));
                    }
                    Neighbor(s) => match **s {
                        YieldSpeed(s) => {
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
                    YieldSize(x) => {
                        blocks.push(comment(format!("_{}_: *x{}* size boost", adv.title, x)));
                    }
                    Neighbor(s) => match **s {
                        YieldSize(s) => {
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
            for &(config::SpawnRate(guard, (lo, hi)), ah) in sum.yields.iter() {
                let arch = match CONFIG.possession_archetypes.get(ah) {
                    Some(arch) => arch,
                    None => {
                        error!("unknown arch in yield {}", ah);
                        continue;
                    }
                };
                let name = &arch.name;

                blocks.push(comment(format!(
                    "{}between *{}* and *{}* {} _{}_",
                    if guard == 1.0 {
                        "".to_string()
                    } else {
                        format!("up to *{:.1}*% chance of ", guard * 100.0)
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
            use config::ApplicationEffect::*;
            use futures::stream::{self, StreamExt, TryStreamExt};

            let (tile_id, applicable_ids): (uuid::Uuid, Vec<uuid::Uuid>) =
                serde_json::from_str(&action.value).unwrap();
            let db = dyn_db();

            let applicables = stream::iter(applicable_ids)
                .map(|id| hacksteader::get_possession(&db, Key::misc(id)))
                .buffer_unordered(50)
                .try_collect::<Vec<Possession>>()
                .await
                .map_err(|e| {
                    let a = format!("couldn't get applicable: {}", e);
                    error!("{}", a);
                    a
                })?
                .into_iter()
                .map(|x| x.try_into())
                .collect::<Result<Vec<Possessed<possess::Keepsake>>, _>>()
                .map_err(|e| {
                    let a = format!("couldn't transform fetched applicable: {}", e);
                    error!("{}", a);
                    a
                })?;

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
                                    "description": plain_text(match i.inner.item_application_effect.as_ref()? {
                                        TimeIncrease { extra_cycles, duration_cycles } => format!(
                                            "~{} hours pass in {} minutes",
                                            extra_cycles / FARM_CYCLES_PER_MIN / 60,
                                            duration_cycles / FARM_CYCLES_PER_MIN,
                                        )
                                    }),
                                    // this is fucky-wucky because value can only be 75 chars
                                    "value": serde_json::to_string(&(
                                        &tile_id.to_simple().to_string(),
                                        i.id.to_simple().to_string(),
                                        i.archetype_handle
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
        "push_possession_page" => {
            let page_json = action.value;
            let mut page: PossessionPage = serde_json::from_str(&page_json).unwrap();

            if page.possession.steader == i.user.id {
                page.credentials = Credentials::Owner;
            }

            page.modal(i.trigger_id, "push").launch().await?
        }
        "possession_page" => {
            let page_json = action.value;
            let mut page: PossessionPage = serde_json::from_str(&page_json).unwrap();

            if page.possession.steader == i.user.id {
                page.credentials = Credentials::Owner;
            }

            page.modal(i.trigger_id, "open").launch().await?
        }
        "possession_overview_page" => {
            let page_json = action.value;
            let page: PossessionOverviewPage = serde_json::from_str(&page_json).unwrap();

            page.modal(i.trigger_id, "open").await?.launch().await?
        }
        _ => mrkdwn("huh?"),
    };

    Ok(ActionResponse::Json(Json(output_json)))
}

#[rocket::get("/steadercount")]
async fn steadercount() -> Result<String, String> {
    core::Profile::fetch_all(&dyn_db())
        .await
        .map(|profiles| profiles.len().to_string())
}

pub enum FarmingInputEvent {
    ActivateUser(String),
    RedeemLandCert(uuid::Uuid, String),
    ApplyItem(uuid::Uuid, config::ArchetypeHandle),
    PlantSeed(uuid::Uuid, hacksteader::Plant),
    BeginCraft {
        tile_id: uuid::Uuid,
        recipe: config::Recipe<config::ArchetypeHandle>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    use rocket_contrib::serve::StaticFiles;

    dotenv::dotenv().ok();
    pretty_env_logger::init();

    info!("starting");

    let (tx, rx) = crossbeam_channel::unbounded();

    tokio::task::spawn({
        use std::collections::HashMap;
        use std::time::{Duration, SystemTime};
        use tokio::time::interval;

        struct ItemEffect {
            // the number of farming cycles this effect lasts for
            duration_cycles_total: u64,

            // the number of extra cycles this effect applies to its plant
            // (right now all items can do is give their plants extra cycles)
            extra_cycles_total: u64,
            // the number of cycles still to be awarded to the plant
            extra_cycles_remaining: u64,
        }

        let mut interval = interval(Duration::from_millis(FARM_CYCLE_MILLIS));

        let mut active_users: HashMap<String, bool> = HashMap::new();
        let mut item_effects: HashMap<uuid::Uuid, ItemEffect> = HashMap::new();
        let mut plant_queue: HashMap<uuid::Uuid, hacksteader::Plant> = HashMap::new();
        let mut craft_queue: HashMap<uuid::Uuid, config::Recipe<_>> = HashMap::new();
        let mut land_cert_queue: HashMap<String, uuid::Uuid> = HashMap::new();

        async move {
            use core::Profile;
            use futures::stream::{self, StreamExt, TryStreamExt};
            use hacksteader::{Plant, Tile};
            use std::collections::HashMap;

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
                        ApplyItem(tile_id, item_ah) => {
                            let config::ApplicationEffect::TimeIncrease {
                                extra_cycles,
                                duration_cycles,
                            } = CONFIG
                                .possession_archetypes
                                .get(item_ah)
                                .expect("invalid archetype handle")
                                .kind
                                .keepsake()
                                .expect("can only apply keepsakes")
                                .item_application_effect
                                .clone()
                                .expect("keepsake has no application effect");

                            item_effects.insert(
                                tile_id,
                                ItemEffect {
                                    duration_cycles_total: duration_cycles,
                                    extra_cycles_total: extra_cycles,
                                    extra_cycles_remaining: extra_cycles,
                                },
                            );
                        }
                        PlantSeed(tile_id, plant) => {
                            plant_queue.insert(tile_id, plant);
                        }
                        RedeemLandCert(cert_id, user_id) => {
                            land_cert_queue.insert(user_id, cert_id);
                        }
                        BeginCraft { tile_id, recipe } => {
                            craft_queue.insert(tile_id, recipe);
                        }
                    }
                }

                interval.tick().await;

                if active_users.is_empty() {
                    continue;
                }

                let db = dyn_db();

                let mut deletions = vec![];
                let mut clear_plants = vec![];
                let mut possessions = vec![];
                let mut new_tiles = vec![];
                let mut dms: Vec<(String, [Value; 2])> = Vec::new();

                let mut hacksteaders: Vec<Hacksteader> = match stream::iter(active_users.clone())
                    .map(|(id, _)| Hacksteader::from_db(&db, id))
                    .buffer_unordered(50)
                    .try_collect::<Vec<_>>()
                    .await
                {
                    Ok(i) => i,
                    Err(e) => {
                        error!("error reading hacksteader from db: {}", e);
                        continue;
                    }
                };

                // Give away requested land
                for hs in hacksteaders.iter_mut() {
                    if let Some(cert_id) = land_cert_queue.remove(&hs.user_id) {
                        deletions.push(Key::misc(cert_id));
                        let new_tile = hacksteader::Tile::new(hs.user_id.clone());
                        hs.land.push(new_tile.clone());
                        new_tiles.push(new_tile.clone());
                    }
                }

                // Launch requested crafts
                for Hacksteader {
                    land, inventory, ..
                } in hacksteaders.iter_mut()
                {
                    for tile in land.iter_mut() {
                        let plant = match &mut tile.plant {
                            Some(pl) => pl,
                            None => continue,
                        };

                        if let Some(recipe) = craft_queue.remove(&tile.id) {
                            let should_take: usize =
                                recipe.needs.iter().map(|(n, _)| n).sum::<usize>();
                            let mut used_resources = recipe
                                .needs
                                .into_iter()
                                .flat_map(|(count, ah)| {
                                    inventory
                                        .iter()
                                        .filter(move |p| p.archetype_handle == ah)
                                        .map(|p| p.key())
                                        .take(count)
                                })
                                .collect::<Vec<_>>();

                            if should_take == used_resources.len() {
                                deletions.append(&mut used_resources);

                                plant.craft = Some(hacksteader::Craft {
                                    until_finish: recipe.time,
                                    total_cycles: recipe.time,
                                    makes: recipe.makes.any(),
                                    destroys_plant: recipe.destroys_plant,
                                });
                            } else {
                                dms.push((
                                    tile.steader.clone(),
                                    [
                                        comment("you don't have enough resources to craft that"),
                                        comment("nice try tho"),
                                    ],
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

                // remove unactive users
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
                    let mut rng = rand::thread_rng();

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

                    // elapsed for this plant is the base amount of time plus some extras
                    // in case something is speeding up time for this plant
                    let boosted_elapsed = {
                        let elapsed = SystemTime::now()
                            .duration_since(profile.last_farm)
                            .unwrap_or_default()
                            .as_millis()
                            / (FARM_CYCLE_MILLIS as u128);

                        // we don't want to add the boosted_elapsed here, then your item effects
                        // would have to be "paid for" later (your farm wouldn't work for however
                        // much time the effect gave you).
                        if elapsed > 0 {
                            if let Some(tokens) = plant_tokens.get_mut(&profile.id) {
                                *tokens = *tokens - 1;
                                if *tokens == 0 {
                                    info!("all plants finished for {}", profile.id);

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

                        if let Some(effect) = item_effects.get(&tile.id).clone() {
                            // decrement counter, remove if 0
                            if effect.extra_cycles_remaining == 0 {
                                info!("removing +{} cyles effect", effect.extra_cycles_total);
                                item_effects.remove(&tile.id);
                                elapsed
                            } else {
                                // calculate award, handle edge case that there's a bit left
                                let base_award =
                                    effect.extra_cycles_total / effect.duration_cycles_total;
                                let to_award = if effect.extra_cycles_remaining < base_award {
                                    effect.extra_cycles_remaining
                                } else {
                                    base_award
                                };

                                let i = item_effects.get_mut(&tile.id).unwrap();
                                i.extra_cycles_remaining -= to_award;

                                elapsed + to_award as u128
                            }
                        } else {
                            elapsed
                        }
                    };

                    info!(
                        "triggering {} farming cycles for {}",
                        boosted_elapsed, profile.id
                    );
                    for _ in 0..boosted_elapsed {
                        let plant_sum = plant
                            .advancements
                            .sum(plant.xp, neighbor_bonuses.iter());

                        plant.craft = match plant.craft.take() {
                            Some(mut craft) => {
                                if craft.until_finish > plant_sum.yield_speed_multiplier {
                                    craft.until_finish -= plant_sum.yield_speed_multiplier;
                                    Some(craft)
                                } else {
                                    let p = Possession::new(
                                        craft.makes,
                                        possess::Owner::crafter(tile.steader.clone()),
                                    );
                                    if craft.destroys_plant {
                                        clear_plants.push(tile.id.clone());
                                    }
                                    possessions.push(p.clone());
                                    dms.push((
                                        tile.steader.clone(),
                                        [
                                            json!({
                                                "type": "section",
                                                "text": mrkdwn(format!(
                                                    "Your *{}* has finished crafting a {} *{}* for you!",
                                                    plant.name,
                                                    emojify(&p.name),
                                                    p.name
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
                                                    "alt_text": "Hackpheus holding a Gift!",
                                                }
                                            }),
                                            comment("YAY FREE STUFFZ 'CEPT LIKE IT'S NOT FREE")
                                        ]
                                    ));
                                    None
                                }
                            }
                            None => None,
                        };

                        plant.until_yield = match plant.until_yield
                            - plant_sum.yield_speed_multiplier
                        {
                            n if n > 0.0 => n,
                            _ => {
                                let mut produced: Vec<Possession> = plant_sum
                                    .yields
                                    .iter()
                                    .flat_map({
                                        let owner = &tile.steader;
                                        let pedigree = &plant.pedigree;
                                        move |(spawn_rate, ah)| {
                                            (0..spawn_rate.gen_count(&mut rng)).map(move |_| {
                                                let mut p = Possession::new(
                                                    *ah,
                                                    possess::Owner::farmer(owner.clone()),
                                                );
                                                if let Some(s) = p.kind.seed_mut() {
                                                    s.pedigree = pedigree.clone();

                                                    if let Some(sg) = s
                                                        .pedigree
                                                        .last_mut()
                                                        .filter(|sg| sg.id == *owner)
                                                    {
                                                        sg.generations += 1;
                                                    } else {
                                                        s.pedigree.push(
                                                            possess::seed::SeedGrower::new(
                                                                owner.clone(),
                                                                1,
                                                            ),
                                                        )
                                                    }
                                                }
                                                p
                                            })
                                        }
                                    })
                                    .collect();

                                dms.push((tile.steader.clone(), [
                                    json!({
                                        "type": "section",
                                        "text": mrkdwn(format!(
                                            concat!(
                                                ":tada: Your *{}* has produced the following for you:\n\n{}\n\n",
                                                ":sparkles: XP Bonus: *{}xp*",
                                            ),
                                            plant.name,
                                            produced
                                                .iter()
                                                .map(|x| format!("{} _{}_", emojify(&x.name), x.name))
                                                .collect::<Vec<String>>()
                                                .join(",\n"),
                                            100
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
                                ]));

                                possessions.append(&mut produced);

                                plant.base_yield_duration
                            }
                        };

                        if let Some(advancement) = plant.increment_xp() {
                            dms.push((tile.steader.clone(), [
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
                            ]))
                        }
                        let profile_sum = profile.advancements.sum(profile.xp, std::iter::empty());
                        if let Some(advancement) = profile.increment_xp() {
                            dms.push((tile.steader.clone(), [
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
                            ]));
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
                                request_items: [(core::TABLE_NAME.to_string(), items.to_vec())]
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
                    stream::iter(dms)
                        .map(|x| Ok(x))
                        .try_for_each_concurrent(None, |(who, blocks)| {
                            dm_blocks(who.clone(), blocks.to_vec())
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
        .serve()
        .await
        .expect("launch fail");

    Ok(())
}
