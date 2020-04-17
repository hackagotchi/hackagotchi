#![feature(decl_macro)]
#![feature(proc_macro_hygiene)]
use rocket::request::LenientForm;
use rocket::{post, routes, FromForm};
use rocket_contrib::json::Json;
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};
use serde_json::{json, Value};

mod banker;
mod hacksteader;

use hacksteader::{Gotchi, Hacksteader, Possessed};

fn dyn_db() -> DynamoDbClient {
    DynamoDbClient::new(rusoto_core::Region::UsEast1)
}

lazy_static::lazy_static! {
    pub static ref TOKEN: String = std::env::var("TOKEN").unwrap();
    pub static ref ID: String = std::env::var("ID").unwrap();
    pub static ref APP_ID: String = std::env::var("APP_ID").unwrap();
    pub static ref URL: String = std::env::var("URL").unwrap();
    pub static ref HOMESTEAD_PRICE: usize = std::env::var("HOMESTEAD_PRICE").unwrap().parse().unwrap();
}

fn mrkdwn<S: std::string::ToString>(txt: S) -> Value {
    json!({
        "type": "mrkdwn",
        "text": txt.to_string(),
    })
}
fn plain_text<S: std::string::ToString>(txt: S) -> Value {
    json!({
        "type": "plain_text",
        "text": txt.to_string(),
    })
}
fn comment<S: ToString>(txt: S) -> Value {
    json!({
        "type": "context",
        "elements": [
            mrkdwn(txt)
        ]
    })
}

async fn dm_blocks(user_id: String, blocks: Vec<Value>) -> Result<(), String> {
    let o = json!({
        "channel": user_id,
        "token": *TOKEN,
        "blocks": blocks
    });

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

#[derive(serde::Serialize, serde::Deserialize)]
pub struct GotchiPage {
    gotchi: Possessed<Gotchi>,
    interactivity: Interactivity,
}

impl GotchiPage {
    fn modal(self, trigger_id: String, page_json: String) -> Modal {
        Modal {
            method: "open".to_string(),
            trigger_id: trigger_id,
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            title: self.gotchi.inner.nickname,
            private_metadata: page_json,
            ..Default::default()
        }
    }

    fn modal_update(self, trigger_id: String, page_json: String, view_id: String) -> ModalUpdate {
        ModalUpdate {
            trigger_id,
            view_id,
            callback_id: self.callback_id(),
            blocks: self.blocks(),
            title: self.gotchi.inner.nickname,
            private_metadata: page_json,
            hash: None,
            submit: None,
        }
    }

    fn callback_id(&self) -> String {
        "gotchi_page_".to_string() + self.interactivity.id()
    }

    fn blocks(&self) -> Vec<Value> {
        // TODO: with_capacity optimization
        let mut blocks: Vec<Value> = Vec::new();
        let Self {
            gotchi,
            interactivity,
        } = self;

        let actions = |buttons: &[&str], value: String| -> Value {
            match interactivity {
                Interactivity::Dynamic => json!({
                    "type": "actions",
                    "elements": buttons.iter().map(|action| json!({
                        "type": "button",
                        "text": plain_text(action),
                        "value": value,
                        "action_id": format!("gotchi_{}", action.to_lowercase()),
                    })).collect::<Vec<_>>()
                }),
                Interactivity::Static => comment("This gotchi page is read only."),
            }
        };

        blocks.push(actions(&["Nickname"], gotchi.inner.nickname.clone()));

        let text = [
            ("species", gotchi.name.clone()),
            ("base happiness", gotchi.inner.base_happiness.to_string()),
            (
                "owner log",
                gotchi
                    .ownership_log
                    .iter()
                    .map(|o| format!("[{}]<@{}>", o.acquisition, o.id))
                    .collect::<Vec<_>>()
                    .join(" -> ")
                    .to_string(),
            ),
        ]
        .iter()
        .map(|(l, r)| format!("*{}:* _{}_", l, r))
        .collect::<Vec<_>>()
        .join("\n");

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(text),
            "accessory": {
                "type": "image",
                "image_url": format!("http://{}/gotchi/img/gotchi/{}.png", *URL, gotchi.name.to_lowercase()),
                "alt_text": "hackagotchi img",
            }
        }));

        blocks.push(actions(&["Trade", "Auction"], gotchi.sk()));

        blocks.push(comment(format!(
            "*Lifetime GP harvested: {}*",
            gotchi
                .inner
                .harvest_log
                .iter()
                .map(|x| x.harvested)
                .sum::<usize>(),
        )));
        for owner in gotchi.inner.harvest_log.iter() {
            blocks.push(comment(format!(
                "{}gp harvested for <@{}>",
                owner.harvested, owner.id
            )));
        }

        blocks
    }
}

async fn update_user_home_tab(user_id: String) -> Result<(), String> {
    update_home_tab(
        Hacksteader::from_db(&dyn_db(), user_id.clone()).await,
        user_id.clone(),
    )
    .await
}
async fn update_home_tab(hs: Option<Hacksteader>, user_id: String) -> Result<(), String> {
    let o = json!({
        "user_id": user_id,
        "view": {
            "type": "home",
            "blocks": hacksteader_greeting_blocks(hs, Interactivity::Dynamic),
        }
    });

    println!("home screen: {}", serde_json::to_string_pretty(&o).unwrap());

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

fn hackstead_blocks(hs: Hacksteader, interactivity: Interactivity) -> Vec<Value> {
    use humantime::format_duration;
    use std::time::SystemTime;

    // TODO: with_capacity optimization
    let mut blocks: Vec<Value> = Vec::new();

    blocks.push(json!({
        "type": "section",
        "text": mrkdwn(format!("*_<@{}>'s Hackstead_*", hs.user_id)),
    }));

    blocks.push(comment(format!(
        "founded {} ago (roughly)",
        format_duration(SystemTime::now().duration_since(hs.profile.joined).unwrap()),
    )));

    blocks.push(json!({ "type": "divider" }));

    if hs.gotchis.len() > 0 {
        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(match hs.gotchis.len() {
                1 => "*Your Hackagotchi*".into(),
                _ => format!("*Your {} Hackagotchi*", hs.gotchis.len())
            }),
        }));

        let total_happiness = hs
            .gotchis
            .iter()
            .map(|g| g.inner.base_happiness)
            .sum::<usize>();

        for gotchi in hs.gotchis.into_iter() {
            blocks.push(json!({
                "type": "section",
                "text": mrkdwn(format!("_:{0}: ({0}, {1} happiness)_", gotchi.name, gotchi.inner.base_happiness)),
                "accessory": {
                    "type": "button",
                    "style": "primary",
                    "text": plain_text(&gotchi.inner.nickname),
                    "value": serde_json::to_string(&GotchiPage {
                        gotchi,
                        interactivity
                    }).unwrap(),
                    "action_id": "gotchi_page",
                }
            }));
        }

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(format!("Total happiness: *{}*", total_happiness))
        }));
        blocks.push(comment(
            "The total happiness of all your gotchi is equivalent to the \
             amount of GP you'll get at the next Harvest.",
        ));
    }

    blocks.push(json!({ "type": "divider" }));

    if let Interactivity::Static = interactivity {
        blocks.push(comment(format!(
            "This is a read-only snapshot of <@{}>'s Hackagotchi Hackstead at a specific point in time. \
            You can manage your own Hackagotchi Homestead in real time at your \
            <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>.",
            hs.user_id,
            *APP_ID,
        )));
    }

    println!(
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
:money_with_wings: Buy, sell and trade Farmables and Hackagotchi at an open auction!

Hacksteading costs *{} GP*.
As a Hacksteader, you'll have a plot of land on which to grow your own Farmables which make Hackagotchi happier. \
Happier Hackagotchi generate more passive income! \
You can also buy, sell, and trade Farmables and Hackagotchi for GP on an open auction space. \
",
$hackstead_cost
) } }

fn hackstead_explanation_blocks() -> Vec<Value> {
    vec![
        json!({
            "type": "section",
            "text": mrkdwn(hacksteader_opening_blurb!(*HOMESTEAD_PRICE)),
        }),
        json!({
            "type": "actions",
            "elements": [{
                "type": "button",
                "action_id": "homestead_confirm",
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
) -> Vec<Value> {
    let o = match hacksteader {
        Some(hs) => hackstead_blocks(hs, interactivity),
        None => hackstead_explanation_blocks(),
    };

    println!("{}", serde_json::to_string_pretty(&o).unwrap());

    o
}

fn hackmarket_blocks() -> Vec<Value> {
    vec![]
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
}

#[post("/hackmarket", data = "<slash_command>")]
async fn hackmarket<'a>(slash_command: LenientForm<SlashCommand>) -> Json<Value> {
    Json(json!({
        "blocks": hackmarket_blocks(),
        "response_type": "in_channel",
    }))
}

#[post("/hackstead", data = "<slash_command>")]
async fn hackstead<'a>(slash_command: LenientForm<SlashCommand>) -> Json<Value> {
    println!("{:#?}", slash_command);

    lazy_static::lazy_static! {
        static ref HACKSTEAD_REGEX: regex::Regex = regex::Regex::new(
            "(<@([A-z0-9]+)|(.+)>)?"
        ).unwrap();
    }

    let captures = HACKSTEAD_REGEX.captures(&slash_command.text);
    println!("captures: {:#?}", captures);
    let user = captures
        .and_then(|c| c.get(2).map(|x| x.as_str()))
        .unwrap_or(&slash_command.user_id);

    let hs = Hacksteader::from_db(&dyn_db(), user.to_string()).await;
    Json(json!({
        "blocks": hacksteader_greeting_blocks(hs, Interactivity::Static),
        "response_type": "in_channel",
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

        println!("{}", serde_json::to_string_pretty(&o).unwrap());
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

        println!("{}", serde_json::to_string_pretty(&o).unwrap());
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
    Static,
    Dynamic,
}
impl Interactivity {
    fn id(self) -> &'static str {
        use Interactivity::*;
        match self {
            Static => "static",
            Dynamic => "dynamic",
        }
    }
}

#[post("/interact", data = "<action_data>")]
async fn action_endpoint(action_data: LenientForm<ActionData>) -> Result<ActionResponse, String> {
    let v = serde_json::from_str::<Value>(&action_data.payload).unwrap();
    println!("action data: {:#?}", v);

    if let Some("view_submission") = v.get("type").and_then(|t| t.as_str()) {
        println!("right type!");
        let view = v.get("view").and_then(|view| {
            let parsed_view = serde_json::from_value::<View>(view.clone()).ok()?;
            let page_json_str = &parsed_view.private_metadata;
            let page: GotchiPage = match serde_json::from_str(&page_json_str) {
                Ok(page) => Some(page),
                Err(e) => {
                    dbg!(format!("couldn't parse {}: {}", page_json_str, e));
                    None
                }
            }?;
            Some((
                parsed_view,
                page,
                v.get("trigger_id")?.as_str()?,
                view.get("state").and_then(|s| s.get("values"))?,
                serde_json::from_value::<User>(v.get("user")?.clone()).ok()?,
            ))
        });
        if let Some((view, mut page, trigger_id, values, user)) = view {
            println!("view state values: {:#?}", values);

            if let Some(Value::String(nickname)) = values
                .get("gotchi_nickname_input")
                .and_then(|i| i.get("gotchi_nickname_set"))
                .and_then(|s| s.get("value"))
            {
                // update the nickname in the DB
                let db = dyn_db();
                db.update_item(rusoto_dynamodb::UpdateItemInput {
                    table_name: hacksteader::TABLE_NAME.to_string(),
                    key: page.gotchi.empty_item(user.id.clone()),
                    update_expression: Some("SET nickname = :new_name".to_string()),
                    condition_expression: Some("attribute_exists(nickname)".to_string()),
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

                // update the nickname on the Gotchi,
                page.gotchi.inner.nickname = nickname.clone();
                let page_json = serde_json::to_string(&page).unwrap();

                // update the page in the background with the new gotchi data
                page.modal_update(trigger_id.to_string(), page_json, view.root_view_id).launch().await?;

                // update the home tab
                // TODO: make this not read from the database
                update_user_home_tab(user.id.clone()).await?;

                // this will close the "enter nickname" modal
                return Ok(ActionResponse::Ok(()));
            } else if let Some(Value::String(new_owner)) = values
                .get("gotchi_trade_input")
                .and_then(|i| i.get("gotchi_trade_confirm"))
                .and_then(|s| s.get("selected_user"))
            {
                println!("trading {} from {} to {}", view.private_metadata, user.id, new_owner);

                if user.id == *new_owner {
                    println!("self trade attempted");

                    return Ok(ActionResponse::Json(Json(json!({
                        "response_action": "errors",
                        "errors": {
                            "gotchi_trade_input": "absolutely not okay",
                        }
                    }))));
                }

                // update the owner in the DB
                Hacksteader::transfer_possession(
                    &dyn_db(), 
                    user.id.clone(), 
                    new_owner.clone(), 
                    hacksteader::Acquisition::Trade,
                    &mut page.gotchi
                ).await?;
                
                // update the home tab
                // TODO: make this not read from the database
                update_user_home_tab(user.id.clone()).await?;

                // DM the new_owner about their new acquisition!
                dm_blocks(new_owner.clone(), {
                    // TODO: with_capacity optimization
                    let mut blocks = vec![
                        json!({
                            "type": "section",
                            "text": mrkdwn(format!(
                                "<@{}> has been so kind as to gift you a Hackagotchi, :{}: _{}_!",
                                user.id,
                                page.gotchi.name,
                                page.gotchi.inner.nickname,
                            ))
                        }),
                        json!({ "type": "divider" }),
                    ];
                    page.interactivity = Interactivity::Static;
                    blocks.append(&mut page.blocks());
                    blocks.push(json!({ "type": "divider" }));
                    blocks.push(comment(format!(
                        "Manage all of your Hackagotchi at your <slack://app?team=T0266FRGM&id={}&tab=home|hackstead>",
                        *APP_ID,
                    )));
                    blocks
                }).await?;


                // close ALL THE MODALS!!!
                return Ok(ActionResponse::Json(Json(json!({
                    "response_action": "clear",
                }))));
            }
        }
    }

    let mut i: Interaction =
        serde_json::from_str(&action_data.payload).map_err(|e| dbg!(format!("bad data: {}", e)))?;

    println!("{:#?}", i);

    Ok(ActionResponse::Json(Json(
        if let Some(action) = i.actions.pop() {
            match action
                .action_id
                .or(action.name)
                .ok_or("no action name".to_string())?
                .as_str()
            {
                "homestead_confirm" => {
                    banker::message(&format!(
                        "<@{}> invoice <@{}> {} for let's homestead, fred!",
                        *banker::ID,
                        i.user.id,
                        *HOMESTEAD_PRICE,
                    ))
                    .await
                    .map_err(|e| format!("couldn't send Banker invoice DM: {}", e))?;

                    mrkdwn("Check your DMs from Banker for the homesteading invoice!")
                }
                "gotchi_trade" => {
                    let page_json = i.view.ok_or("no view!".to_string())?.private_metadata;
                    let page: GotchiPage = serde_json::from_str(&page_json)
                        .map_err(|e| dbg!(format!("couldn't parse {}: {}", page_json, e)))?;

                    Modal {
                        method: "push".to_string(),
                        trigger_id: i.trigger_id,
                        callback_id: "gotchi_trade_modal".to_string(),
                        title: "Trade Gotchi".to_string(),
                        blocks: vec![json!({
                            "type": "input",
                            "block_id": "gotchi_trade_input",
                            "label": plain_text("Trade Gotchi"),
                            "element": {
                                "type": "users_select",
                                "action_id": "gotchi_trade_confirm",
                                "placeholder": plain_text("Who Gets your Gotchi?"),
                                "initial_user": ({
                                    let s = &hacksteader::CONFIG.special_users;
                                    &s.get(page_json.len() % s.len()).unwrap_or(&*ID)
                                }),
                                "confirm": {
                                    "title": plain_text("You sure?"),
                                    "text": mrkdwn(format!(
                                        "Are you sure you want to give away :{}: _{}_? You might not get them back. :frowning:",
                                        page.gotchi.name,
                                        page.gotchi.inner.nickname
                                    )),
                                    "confirm": plain_text("Give!"),
                                    "deny": plain_text("No!"),
                                    "style": "danger",
                                }
                            }
                        })],
                        private_metadata: page_json,
                        submit: Some("Trade Away!".to_string()),
                        ..Default::default()
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
                            "block_id": "gotchi_nickname_input",
                            "label": plain_text("Nickname Gotchi"),
                            "element": {
                                "type": "plain_text_input",
                                "action_id": "gotchi_nickname_set",
                                "placeholder": plain_text("Nickname Gotchi"),
                                "initial_value": action.value,
                                "min_length": 1,
                                "max_length": 41,
                            }
                        })],
                        submit: Some("Change it!".to_string()),
                        ..Default::default()
                    }
                    .launch()
                    .await?
                }
                "gotchi_page" => {
                    let page_json = action.value;
                    let page: GotchiPage = serde_json::from_str(&page_json).unwrap();

                    page.modal(i.trigger_id, page_json).launch().await?
                }
                _ => mrkdwn("huh?"),
            }
        } else {
            mrkdwn("no actions?")
        },
    )))
}

#[derive(serde::Deserialize, Debug)]
struct ChallengeEvent {
    challenge: String,
}
#[post("/event", format = "application/json", data = "<event_data>", rank = 2)]
async fn challenge(event_data: Json<ChallengeEvent>) -> String {
    event_data.challenge.clone()
}

#[derive(serde::Deserialize, Debug, Clone)]
struct Event<'a> {
    #[serde(borrow, rename = "event")]
    reply: Reply<'a>,
}
#[derive(serde::Deserialize, Debug, Clone)]
pub struct Reply<'a> {
    #[serde(rename = "user")]
    user_id: String,
    channel: String,
    #[serde(default)]
    text: String,
    #[serde(rename = "type")]
    kind: &'a str,
    tab: Option<&'a str>,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
async fn event(e: Json<Event<'_>>) -> Result<(), String> {
    use hacksteader::CONFIG;

    let Event { reply: r } = (*e).clone();
    println!("{:#?}", r);

    lazy_static::lazy_static! {
        static ref SPAWN_POSSESSION_REGEX: regex::Regex = regex::Regex::new(
            "<@([A-z|0-9]+)> spawn (<@([A-z|0-9]+)> )?([A-z]+)"
        ).unwrap();
    }

    // TODO: clean these three mofos up
    if let Reply {
        kind: "app_home_opened",
        tab: Some("home"),
        user_id,
        ..
    } = r
    {
        println!("Rendering app_home!");
        update_user_home_tab(user_id).await?;
    } else if let Some(paid_invoice) =
        banker::parse_paid_invoice(&r).filter(|pi| pi.invoicer == *ID)
    {
        println!("{} just paid an invoice", paid_invoice.invoicee);
        Hacksteader::new_in_db(&dyn_db(), paid_invoice.invoicee.clone())
            .await
            .map_err(|_| "Couldn't put you in the hacksteader database!")?;
    } else if CONFIG.special_users.contains(&r.user_id) {
        if let Some((receiver, archetype_handle)) =
            SPAWN_POSSESSION_REGEX.captures(&r.text).and_then(|c| {
                let _spawner = c.get(1).filter(|x| x.as_str() == &*ID)?;
                let receiver = c.get(3).map(|x| x.as_str()).unwrap_or(&r.user_id);
                let possession_name = c.get(4)?.as_str();
                let archetype_handle = CONFIG
                    .archetypes
                    .iter()
                    .position(|x| x.name == possession_name)?;
                Some((receiver, archetype_handle))
            })
        {
            Hacksteader::give_possession_from_archetype(
                &dyn_db(),
                receiver.to_string(),
                archetype_handle,
            )
            .await
            .map_err(|_| "hacksteader database problem")?;
        }
    }

    Ok(())
}

fn main() {
    use rocket_contrib::serve::StaticFiles;

    dotenv::dotenv().ok();

    rocket::ignite()
        .mount(
            "/gotchi",
            routes![hackstead, hackmarket, action_endpoint, challenge, event],
        )
        .mount(
            "/gotchi/img",
            StaticFiles::from(concat!(env!("CARGO_MANIFEST_DIR"), "/img")),
        )
        .launch()
        .expect("launch fail");
}
