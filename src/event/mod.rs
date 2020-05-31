use crate::{ID, update_user_home_tab};
use rocket_contrib::json::Json;
use crossbeam_channel::Sender;
use std::pin::Pin;
use super::banker;
use regex::Regex;
use rocket::{post, State};
use std::future::Future;

mod banker_message;
mod invoice_payment;
mod special_user_message;

mod prelude {
    // std/util
    pub use crossbeam_channel::Sender;
    pub use std::convert::TryInto;
    pub use serde_json::{json, Value};
    pub use regex::Regex;
    pub use log::*;
    // db
    pub use rusoto_dynamodb::{AttributeValue, DynamoDb};
    pub use crate::dyn_db;
    // futures
    pub use futures::future::{TryFutureExt, FutureExt};
    pub use futures::stream::{self, StreamExt, TryStreamExt};
    // us
    pub use super::{Trigger, HandlerOutput, Message};
    pub use crate::{FarmingInputEvent, URL};
    pub use core::{Category, Key};
    pub use core::possess;
    pub use possess::{Possessed, Possession, Gotchi, Seed, Keepsake};
    pub use core::config;
    pub use config::CONFIG;
    pub use crate::{banker, hacksteader, market};
    pub use hacksteader::Hacksteader;
    // slack frontend
    pub use crate::{comment, dm_blocks, filify, mrkdwn};
    pub use core::frontend::emojify;
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
pub struct Event<'a> {
    #[serde(borrow, rename = "event")]
    reply: Message<'a>,
}
#[derive(serde::Deserialize, Debug, Clone)]
pub struct Message<'a> {
    #[serde(rename = "user")]
    pub user_id: String,
    pub channel: String,
    #[serde(default)]
    pub text: String,
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub tab: Option<&'a str>,
}

#[post("/event", format = "application/json", data = "<e>", rank = 1)]
pub async fn non_challenge_event(
    to_farming: State<'_, Sender<FarmingInputEvent>>,
    e: Json<Event<'_>>,
) -> Result<(), String> {
    use super::banker::parse_paid_invoice;

    let Event { reply: ref r } = *e;
    debug!("{:#?}", r);

    // TODO: clean these three mofos up
    if let &Message {
        kind: "app_home_opened",
        tab: Some("home"),
        ref user_id,
        ..
    } = r
    {
        info!("Rendering app_home!");
        to_farming
            .send(crate::FarmingInputEvent::ActivateUser(user_id.clone()))
            .expect("couldn't send active user");
        update_user_home_tab(user_id.clone()).await?;
    } else if let Some(ref pi) = parse_paid_invoice(&r).filter(|p| p.invoicer == *ID) {
        info!("invoice {:#?} just paid", pi);

        for InvoicePaymentTrigger { regex, then } in INVOICE_PAYMENT_TRIGGERS.iter() {
            let c = match regex.captures(&pi.reason) {
                Some(c) => c,
                None => continue,
            };
            if let Err(e) = then(c, r.clone(), pi.clone()).await {
                banker::message(
                    format!("invoice payment handler err : {}", e)
                ).await?;
            }
        }
    } else if core::config::CONFIG.special_users.contains(&r.user_id) {
        for SpecialUserMessageTrigger { regex, then } in SPECIAL_USER_MESSAGE_TRIGGERS.iter() {
            let c = match regex.captures(&r.text) {
                Some(c) => c,
                None => continue,
            };
            if let Err(e) = then(c, r.clone(), &*to_farming).await {
                banker::message(
                    format!("special user handler err : {}", e)
                ).await?;
            }
        }
    } else if r.channel == *banker::CHAT_ID {
        for BankerMessageTrigger { regex, then } in BANKER_MESSAGE_TRIGGERS.iter() {
            let c = match regex.captures(&r.text) {
                Some(c) => c,
                None => continue,
            };
            if let Err(e) = then(c, r.clone(), &*to_farming).await {
                banker::message(
                    format!("banker message handler err : {}", e)
                ).await?;
            }
        }
    }

    Ok(())
}

pub type HandlerOutput<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + 'a + Send>>;
pub type CaptureHandler = dyn for<'a> Fn(regex::Captures<'a>, Message<'a>, &'a Sender<FarmingInputEvent>) -> HandlerOutput<'a> + 'static + Sync + Send;
pub type PaidInvoiceHandler = dyn for<'a> Fn(regex::Captures<'a>, Message<'a>, banker::PaidInvoice) -> HandlerOutput<'a> + 'static + Sync + Send;
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
