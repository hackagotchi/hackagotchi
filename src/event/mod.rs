use super::banker;
use crate::{update_user_home_tab, ID};
use crossbeam_channel::Sender;
use regex::Regex;
use rocket::{post, State};
use rocket_contrib::json::Json;
use std::future::Future;
use std::pin::Pin;

mod banker_message;
mod invoice_payment;
mod special_user_message;

mod prelude {
    // std/util
    pub use crossbeam_channel::Sender;
    pub use log::*;
    pub use regex::Regex;
    pub use serde_json::{json, Value};
    pub use std::convert::TryInto;
    // db
    pub use crate::dyn_db;
    pub use rusoto_dynamodb::{AttributeValue, DynamoDb};
    // futures
    pub use futures::future::{FutureExt, TryFutureExt};
    pub use futures::stream::{self, StreamExt, TryStreamExt};
    // us
    pub use super::{HandlerOutput, Message, Trigger};
    pub use crate::{banker, hacksteader, market};
    pub use crate::{FarmingInputEvent, URL};
    pub use config::CONFIG;
    pub use hacksteader::Hacksteader;
    pub use hcor::config;
    pub use hcor::possess;
    pub use hcor::{Category, Key};
    pub use possess::{Gotchi, Keepsake, Possessed, Possession, Seed};
    // slack frontend
    pub use crate::{comment, dm_blocks, filify, mrkdwn};
    pub use hcor::frontend::emojify;
}
use prelude::*;

#[derive(serde::Deserialize, Debug)]
pub struct ChallengeEvent {
    challenge: String,
}
#[post("/event", format = "application/json", data = "<event_data>", rank = 2)]
pub async fn challenge(event_data: Json<ChallengeEvent>) -> String {
    info!("challenge");
    event_data.challenge.clone()
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Event {
    event: Message,
}
#[derive(serde::Deserialize, Debug, Clone)]
pub struct Message {
    #[serde(rename = "user")]
    pub user_id: String,
    pub channel: String,
    #[serde(default)]
    pub text: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub tab: Option<String>,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
pub async fn non_challenge_event<'a, 'b>(
    to_farming: State<'a, Sender<FarmingInputEvent>>,
    e: Json<Event>,
) -> Result<(), String> {
    use super::banker::parse_paid_invoice;

    let Event { event: r } = (*e).clone();

    let to_farming = (*to_farming).clone();
    rocket::tokio::spawn(async move {
        let kind_tab_id = (
            r.kind.as_str(),
            r.tab.as_ref().map(|x| x.as_str()),
            &r.user_id,
        );
        if let ("app_home_opened", Some("home"), user_id) = kind_tab_id {
            info!("Rendering app_home!");
            to_farming
                .send(crate::FarmingInputEvent::ActivateUser(user_id.clone()))
                .expect("couldn't send active user");
            update_user_home_tab(user_id.clone())
                .await
                .unwrap_or_else(|e| error!("{}", e));
        } else if let Some(ref pi) = parse_paid_invoice(&r).filter(|p| p.invoicer == *ID) {
            info!("invoice {:#?} just paid", pi);

            for InvoicePaymentTrigger { regex, then } in INVOICE_PAYMENT_TRIGGERS.iter() {
                let c = match regex.captures(&pi.reason) {
                    Some(c) => c,
                    None => continue,
                };
                if let Err(e) = then(c, r.clone(), pi.clone()).await {
                    banker::message(format!("invoice payment handler err : {}", e))
                        .await
                        .unwrap_or_else(|e| error!("{}", e));
                }
            }
        } else if hcor::config::CONFIG.special_users.contains(&r.user_id) {
            for SpecialUserMessageTrigger { regex, then } in SPECIAL_USER_MESSAGE_TRIGGERS.iter() {
                let c = match regex.captures(&r.text) {
                    Some(c) => c,
                    None => continue,
                };
                if let Err(e) = then(c, r.clone(), &to_farming).await {
                    banker::message(format!("special user handler err : {}", e))
                        .await
                        .unwrap_or_else(|e| error!("{}", e));
                }
            }
        } else if r.channel == *banker::CHAT_ID {
            for BankerMessageTrigger { regex, then } in BANKER_MESSAGE_TRIGGERS.iter() {
                let c = match regex.captures(&r.text) {
                    Some(c) => c,
                    None => continue,
                };
                if let Err(e) = then(c, r.clone(), &to_farming).await {
                    banker::message(format!("banker message handler err : {}", e))
                        .await
                        .unwrap_or_else(|e| error!("{}", e));
                }
            }
        }
    });

    Ok(())
}

pub type HandlerOutput<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + 'a + Send>>;
pub type CaptureHandler = dyn for<'a> Fn(regex::Captures<'a>, Message, &'a Sender<FarmingInputEvent>) -> HandlerOutput<'a>
    + 'static
    + Sync
    + Send;
pub type PaidInvoiceHandler = dyn for<'a> Fn(regex::Captures<'a>, Message, banker::PaidInvoice) -> HandlerOutput<'a>
    + 'static
    + Sync
    + Send;
pub struct Trigger<T> {
    regex: Regex,
    then: T,
}
pub type SpecialUserMessageTrigger = Trigger<&'static CaptureHandler>;
pub type InvoicePaymentTrigger = Trigger<&'static PaidInvoiceHandler>;
pub type BankerMessageTrigger = Trigger<&'static CaptureHandler>;

lazy_static::lazy_static! {
    static ref SPECIAL_USER_MESSAGE_TRIGGERS: [&'static SpecialUserMessageTrigger; 6] = [
        &*special_user_message::SPAWN_COMMAND,
        &*special_user_message::GP_DUMP_COMMAND,
        &*special_user_message::STOMP_COMMAND,
        &*special_user_message::SLAUGHTER_COMMAND,
        &*special_user_message::NAB_COMMAND,
        &*special_user_message::YANK_CONFIG,
    ];
    static ref INVOICE_PAYMENT_TRIGGERS: [&'static InvoicePaymentTrigger; 3] = [
        &*invoice_payment::HACKMARKET_FEES,
        &*invoice_payment::HACKMARKET_PURCHASE,
        &*invoice_payment::START_HACKSTEAD_INVOICE_PAYMENT,
    ];
    static ref BANKER_MESSAGE_TRIGGERS: [&'static BankerMessageTrigger; 1] = [
        &*banker_message::BANKER_BALANCE,
    ];
}
