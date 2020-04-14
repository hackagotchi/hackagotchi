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
    pub static ref URL: String = std::env::var("URL").unwrap();
    pub static ref HOMESTEAD_PRICE: usize = std::env::var("HOMESTEAD_PRICE").unwrap().parse().unwrap();
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

fn mrkdwn<S: std::string::ToString>(txt: S) -> Value {
    json!({
        "type": "mrkdwn",
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

fn render_hackstead(hs: &Hacksteader) -> Value {
    use humantime::format_duration;
    use std::time::SystemTime;

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

        for gotchi in hs.gotchis.iter() {
            blocks.push(json!({
                "type": "section",
                "text": mrkdwn(format!("_:{0}: ({0}, {1} happiness)_", gotchi.name, gotchi.inner.base_happiness)),
                "accessory": {
                    "type": "button",
                    "text": {
                        "text": gotchi.inner.nickname,
                        "type": "plain_text",
                    },
                    "value": serde_json::to_string(&gotchi).unwrap(),
                    "action_id": "gotchi_page",
                }
            }));
        }

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(format!(
                "Total happiness: *{}*",
                hs.gotchis.iter().map(|g| g.inner.base_happiness).sum::<usize>()
            ))
        }));
        blocks.push(comment(
            "The total happiness of all your gotchi is equivalent to the \
             amount of GP you'll get at the next Harvest.",
        ));
    }

    blocks.push(json!({ "type": "divider" }));

    println!(
        "{}",
        serde_json::to_string_pretty(&json!( { "blocks": blocks.clone() })).unwrap()
    );

    json!({ "blocks": blocks })
}

macro_rules! hacksteader_opening_blurb { ( $hackstead_cost:expr ) => { format!(
r#"
*Your Own Hackagotchi Homestead!*

:corn: Grow your own Farmables which make Hackagotchi happier!
:sparkling_heart: Earn passive income by collecting adorable Hackagotchi!
:money_with_wings: Buy, sell and trade Farmables and Hackagotchi at an open auction!

Hacksteading costs *{} GP*.
As a Hacksteader, you'll have a plot of land on which to grow your own Farmables, which can be fed to
Hackagotchi to make them happier. Happier Hackagotchi generate more passive income!
You can also buy, sell, and trade Farmables and Hackagotchi on an open auction space.
"#,
$hackstead_cost
) } }

fn render_hackstead_explanation() -> Value {
    json!(
        {
            "text": hacksteader_opening_blurb!(*HOMESTEAD_PRICE),
            "attachments": [ {
                "text": "Monopolize on Adorableness?",
                "fallback": "You are unable to homestead at the moment.",
                "callback_id": "homestead",
                "attachment_type": "default",
                "actions": [ {
                    "name": "homestead_confirm",
                    "text": "Hack Yeah!",
                    "style": "danger",
                    "type": "button",
                    "value": "confirmed",
                    "confirm": {
                        "title": "Do you have what it takes",
                        "text": "to be a Hackagotchi Homesteader?",
                        "ok_text": "LET'S HOMESTEAD, FRED!",
                        "dismiss_text": "I'm short on GP"
                    }
                } ]
            } ]
        }
    )
}

/// Returns Slack JSON displaying someone's hackstead if they're
/// registered, if not, this command will greet them with an explanation
/// of what hacksteading is and how they can get a hackstead of their own.
fn render_hacksteader_greeting(hacksteader: Option<Hacksteader>) -> Value {
    match hacksteader {
        Some(hs) => render_hackstead(&hs),
        None => render_hackstead_explanation(),
    }
}

#[post("/homestead", data = "<slash_command>")]
async fn homestead<'a>(slash_command: LenientForm<SlashCommand>) -> Json<Value> {
    println!("{:#?}", slash_command);

    let hs = Hacksteader::from_db(&dyn_db(), slash_command.user_id.clone()).await;
    Json(render_hacksteader_greeting(hs))
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
}
#[derive(serde::Deserialize, Debug)]
pub struct User {
    id: String,
}
#[derive(serde::Deserialize, Debug)]
pub struct Action {
    action_id: Option<String>,
    name: Option<String>,
    value: String,
}

async fn modal(
    method: &str,
    trigger_id: &str,
    callback_id: &str,
    title: &str,
    private_metadata: &str,
    blocks: Vec<Value>,
    submit: Option<&str>,
) -> Result<Value, String> {
    let mut o = json!({
        "trigger_id": trigger_id,
        "view": {
            "type": "modal",
            "private_metadata": private_metadata,
            "callback_id": callback_id,
            "title": {
                "type": "plain_text",
                "text": title,
            },
            "blocks": blocks
        }
    });

    if let Some(submit_msg) = submit {
        o["view"].as_object_mut().unwrap().insert(
            "submit".to_string(),
            json!({
                "type": "plain_text",
                "text": submit_msg,
            }),
        );
    }

    let client = reqwest::Client::new();
    client
        .post(&format!("https://slack.com/api/views.{}", method))
        .bearer_auth(&*TOKEN)
        .json(&o)
        .send()
        .await
        .map_err(|e| format!("couldn't open modal: {}", e))?;

    println!("{}", serde_json::to_string_pretty(&o).unwrap());
    Ok(o)
}

#[derive(rocket::Responder)]
pub enum ActionResponse {
    Json(Json<Value>),
    Ok(()),
}

#[post("/interact", data = "<action_data>")]
async fn action_endpoint(action_data: LenientForm<ActionData>) -> Result<ActionResponse, String> {
    let v = serde_json::from_str::<Value>(&action_data.payload).unwrap();
    println!("action data: {:#?}", v);

    if let Some("view_submission") = v.get("type").and_then(|t| t.as_str()) {
        println!("right type!");
        let view = v.get("view").and_then(|view| {
            Some((
                serde_json::from_value::<View>(view.clone()).ok()?,
                view.get("state").and_then(|s| s.get("values"))?,
                serde_json::from_value::<User>(v.get("user")?.clone()).ok()?,
            ))
        });
        if let Some((view, values, user)) = view {
            println!("view state values: {:#?}", values);
            if let Some(Value::String(nickname)) = values
                .get("gotchi_nickname_input")
                .and_then(|i| i.get("gotchi_nickname_set"))
                .and_then(|s| s.get("value"))
            {
                let db = dyn_db();
                db.update_item(rusoto_dynamodb::UpdateItemInput {
                    table_name: hacksteader::TABLE_NAME.to_string(),
                    key: Possessed::<Gotchi>::empty_item_from_parts(
                        view.private_metadata,
                        user.id.clone(),
                    ),
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
                .map_err(|e| format!("Couldn't store nickname change: {}", e))?;

                return Ok(ActionResponse::Ok(()));
            }
        }
    }

    let mut i: Interaction =
        serde_json::from_str(&action_data.payload).map_err(|e| format!("bad data: {}", e))?;

    println!("{:#?}", i);

    Ok(ActionResponse::Json(Json(if let Some(action) = i.actions.pop() {
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
            "gotchi_nickname" => {
                let mut blocks = Vec::new();

                blocks.push(json!({
                    "type": "input",
                    "block_id": "gotchi_nickname_input",
                    "label": {
                        "type": "plain_text",
                        "text": "Nickname Gotchi",
                    },
                    "element": {
                        "type": "plain_text_input",
                        "action_id": "gotchi_nickname_set",
                        "placeholder": {
                            "type": "plain_text",
                            "text": "Nickname Gotchi",
                        },
                        "initial_value": action.value,
                        "min_length": 1,
                        "max_length": 41,
                    }
                }));

                modal(
                    "push",
                    &i.trigger_id,
                    "gotchi_nickname_modal",
                    "Nickname Gotchi",
                    &i.view.ok_or("no view!".to_string())?.private_metadata,
                    blocks,
                    Some("Change it!"),
                )
                .await?
            }
            "gotchi_page" => {
                let mut blocks: Vec<Value> = Vec::new();

                fn actions(buttons: &[&str], value: String) -> Value {
                    json!({
                        "type": "actions",
                        "elements": buttons.iter().map(|action| json!({
                            "type": "button",
                            "text": {
                                "text": action,
                                "type": "plain_text",
                            },
                            "value": value,
                            "action_id": format!("gotchi_{}", action.to_lowercase()),
                        })).collect::<Vec<_>>()
                    })
                }

                let gotchi: Possessed<Gotchi> = serde_json::from_str(&action.value).unwrap();

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

                modal(
                    "open",
                    &i.trigger_id,
                    "gotchi_page",
                    &gotchi.inner.nickname,
                    &gotchi.sk(),
                    blocks,
                    None,
                )
                .await?
            }
            _ => mrkdwn("huh?"),
        }
    } else {
        mrkdwn("no actions?")
    })))
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
struct Event {
    event: serde_json::Value,
}
#[derive(serde::Deserialize, Debug)]
pub struct Reply {
    #[serde(rename = "user")]
    user_id: String,
    channel: String,
    text: String,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
async fn event(e: Json<Event>) -> Result<(), String> {
    use hacksteader::CONFIG;

    let Event { event } = (*e).clone();
    println!("{:#?}", event);

    let r = serde_json::from_value::<Reply>(event).ok();

    lazy_static::lazy_static! {
        static ref SPAWN_POSSESSION_REGEX: regex::Regex = regex::Regex::new(
            "<@([A-z|0-9]+)> spawn (<@([A-z|0-9]+)> )?([A-z]+)"
        ).unwrap();
    }

    // TODO: clean these two mofos up
    if let Some(paid_invoice) = r
        .as_ref()
        .and_then(banker::parse_paid_invoice)
        .filter(|pi| pi.invoicer == *ID)
    {
        println!("{} just paid an invoice", paid_invoice.invoicee);
        Hacksteader::new_in_db(&dyn_db(), paid_invoice.invoicee)
            .await
            .map_err(|_| "Couldn't put you in the hacksteader database!")?;
    } else if let Some((receiver, archetype_handle)) = r
        .filter(|r| CONFIG.special_users.contains(&r.user_id))
        .as_ref()
        .and_then(|r| {
            let c = SPAWN_POSSESSION_REGEX.captures(&r.text)?;
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

    Ok(())
}

fn main() {
    use rocket_contrib::serve::StaticFiles;

    dotenv::dotenv().ok();

    rocket::ignite()
        .mount(
            "/gotchi",
            routes![homestead, action_endpoint, challenge, event],
        )
        .mount(
            "/gotchi/img",
            StaticFiles::from(concat!(env!("CARGO_MANIFEST_DIR"), "/img")),
        )
        .launch()
        .expect("launch fail");
}
