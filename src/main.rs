#![feature(decl_macro)]
#![feature(proc_macro_hygiene)]
use rocket::request::LenientForm;
use rocket::{post, routes, FromForm};
use rocket_contrib::json::Json;
use rusoto_dynamodb::DynamoDbClient;
use serde_json::{json, Value};

mod banker;
mod hacksteader;

fn dyn_db() -> DynamoDbClient {
    DynamoDbClient::new(rusoto_core::Region::UsEast1)
}

lazy_static::lazy_static! {
    pub static ref TOKEN: String = std::env::var("TOKEN").unwrap();
    pub static ref ID: String = std::env::var("ID").unwrap();
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

fn render_hacksteader(

#[post("/homestead", data = "<slash_command>")]
async fn homestead<'a>(
    slash_command: LenientForm<SlashCommand>,
) -> Result<Json<Value>, &'static str> {
    use rusoto_dynamodb::{DynamoDb, GetItemInput};

    println!("{:#?}", slash_command);

    let db = dyn_db();
    let hacksteader = db
        .get_item(GetItemInput {
            key: hacksteader::HacksteaderProfile::empty_item(slash_command.user_id.clone()),
            table_name: hacksteader::TABLE_NAME.to_string(),
            ..Default::default()
        })
        .await
        .map_err(|_| "couldn't reach database")?
        .item
        .as_ref()
        .map(hacksteader::HacksteaderProfile::from_item);

    if let Some(fetched_hacksteader) = hacksteader {
        if let Some(parsed_hacksteader) = fetched_hacksteader {
            Ok(Json(json!({
                "blocks": [
                    {
                        "type": "section",
                        "text": format!("{}", parsed_hacksteader),
                    },
                    {
                        "type": "divider",
                    },
                ]
            }))
        } else {
            Err("This is real bad; we found you in the database but couldn't parse you")
        }
    } else {
        Ok(Json(json!(
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
        )))
    }
}

#[allow(dead_code)]
#[derive(FromForm, Debug)]
struct ActionData {
    payload: String,
}

#[post("/interact", data = "<action_data>")]
async fn action_endpoint(action_data: LenientForm<ActionData>) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(&action_data.payload).map_err(|_| "bad data".to_string())?;

    println!("{:#?}", v);

    match v.get("callback_id") {
        Some(&Value::String(ref s)) if s == "homestead" => {
            let user_id = v
                .get("user")
                .and_then(|x| x.get("id").and_then(|x| x.as_str()))
                .ok_or("no user".to_string())?;

            banker::message(&format!(
                "<@{}> invoice <@{}> {} for let's homestead, fred!",
                *banker::ID,
                user_id,
                *HOMESTEAD_PRICE,
            ))
            .await
            .map_err(|_| "couldn't send Banker invoice DM".to_string())?;

            Ok(format!(
                "Check your DMs from Banker for the homesteading invoice!"
            ))
        }
        _ => Ok("huh?".into()),
    }
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
pub struct ThreadedReply {
    user: String,
    channel: String,
    text: String,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
async fn event(e: Json<Event>) -> Result<(), String> {
    let Event { event } = (*e).clone();
    println!("{:#?}", event);

    if let Some(paid_invoice) = serde_json::from_value::<ThreadedReply>(event)
        .ok()
        .and_then(banker::parse_paid_invoice)
    {
        use rusoto_dynamodb::{DynamoDb, PutItemInput};

        if paid_invoice.invoicer == *ID {
            println!("{} just paid an invoice", paid_invoice.invoicee);
            let hs = hacksteader::HacksteaderProfile::new(paid_invoice.invoicee);
            let db = dyn_db();
            db.put_item(PutItemInput {
                item: hs.item(),
                table_name: hacksteader::TABLE_NAME.to_string(),
                ..Default::default()
            })
            .await
            .map_err(|_| "Couldn't put you in the hacksteader database!")?;
        }
    }

    Ok(())
}

fn main() {
    dotenv::dotenv().ok();

    rocket::ignite()
        .mount(
            "/gotchi",
            routes![homestead, action_endpoint, challenge, event],
        )
        .launch()
        .expect("launch fail");
}
