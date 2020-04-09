#![feature(decl_macro)]
#![feature(proc_macro_hygiene)]
use rocket::request::LenientForm;
use rocket::{post, routes, FromForm};
use rocket_contrib::json::Json;
use rusoto_dynamodb::DynamoDbClient;
use serde_json::{json, Value};

mod banker;
mod hacksteader;

use hacksteader::{Gotchi, Hacksteader};

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

fn render_hackstead(hs: &Hacksteader) -> Value {
    use humantime::format_duration;
    use std::time::SystemTime;

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
                "type": "context",
                "elements": [
                    {
                        "type": "image",
                        "image_url": format!("http://{}/gotchi/img/gotchi/{}.png", *URL, gotchi.archetype_handle),
                        "alt_text": "hackagotchi img",
                    },
                    mrkdwn(format!("_{} ({} power)_", gotchi.name, gotchi.inner.power))
                ]
            }));
        }

        blocks.push(json!({
            "type": "section",
            "text": mrkdwn(format!(
                "Total power: *{}*",
                hs.gotchis.iter().map(|g| g.inner.power).sum::<usize>()
            ))
        }));
        blocks.push(comment(
            "The amount of power you have is equivalent to the \
                            amount of GP you'll get at the next Harvest. This \
                            number is the sum of the power of all of your Hackagotchi.",
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

            Ok("Check your DMs from Banker for the homesteading invoice!".into())
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
pub struct Reply {
    #[serde(rename = "user")]
    user_id: String,
    channel: String,
    text: String,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
async fn event(e: Json<Event>) -> Result<(), String> {
    let Event { event } = (*e).clone();
    println!("{:#?}", event);

    let r = serde_json::from_value::<Reply>(event).ok();

    if let Some(paid_invoice) = r
        .as_ref()
        .and_then(banker::parse_paid_invoice)
        .filter(|pi| pi.invoicer == *ID)
    {
        println!("{} just paid an invoice", paid_invoice.invoicee);
        Hacksteader::new_in_db(&dyn_db(), paid_invoice.invoicee)
            .await
            .map_err(|_| "Couldn't put you in the hacksteader database!")?;
    }

    lazy_static::lazy_static! {
        static ref GIVE_GOTCHI_REGEX: regex::Regex = regex::Regex::new(
            "<@([A-z|0-9]+)> give (<@([A-z|0-9]+)> )?([A-z]+)"
        ).unwrap();
    }

    if let Some(captures) = r.as_ref().and_then(|r| GIVE_GOTCHI_REGEX.captures(&r.text)) {
        dbg!(captures);
        return Ok(());
        /*
        println!("some reply found");
        if text.contains("Adorpheus") {
            Hacksteader::add_gotchi(&dyn_db(), user_id.into(), Gotchi::new("Adorpheus".into(), 3))
                .await
                .map_err(|_| "hacksteader database problem")?;
        }*/
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
