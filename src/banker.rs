use crate::{event::Message, ID as BOT_ID, TOKEN};
use log::{debug, info};
use regex::Regex;

use graphql_client::{GraphQLQuery, QueryBody, Response};
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub struct PaidInvoice {
    pub invoicer: String,
    pub amount: u64,
    pub invoicee: String,
    pub reason: String,
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "hn/schema.json",
    query_path = "hn/create_transaction.graphql"
)]
pub struct CreateTransaction;

use std::env::var;
lazy_static::lazy_static! {
    pub static ref ID: String = var("BANKER_ID").unwrap();
    pub static ref CHAT_ID: String = var("BANKER_CHAT").unwrap();
    pub static ref HN_TOKEN: String = var("HN_TOKEN").unwrap();
    static ref PAID_INVOICE_MSG_REGEX: Regex = Regex::new(
        "<@([A-z|0-9]+)> paid their invoice of ([0-9]+) gp from <@([A-z|0-9]+)> for \"(.+)\"."
    ).unwrap();
}

/// Takes a threaded reply, returns `Some` if that threaded reply contains
/// an invoice payment confirmation message
pub fn parse_paid_invoice(msg: &Message) -> Option<PaidInvoice> {
    if msg.channel == *CHAT_ID && msg.user_id == *ID {
        let caps = dbg!(PAID_INVOICE_MSG_REGEX.captures(&msg.text))?;
        return Some(PaidInvoice {
            invoicee: caps.get(1)?.as_str().to_string(),
            amount: caps.get(2)?.as_str().parse().ok()?,
            invoicer: caps.get(3)?.as_str().to_string(),
            reason: caps.get(4)?.as_str().to_string(),
        });
    }
    None
}

pub async fn message(msg: String) -> Result<(), String> {
    let client = reqwest::Client::new();
    client
        .post("https://slack.com/api/chat.postMessage")
        .form(&[
            ("token", TOKEN.clone()),
            ("channel", CHAT_ID.clone()),
            ("text", msg),
        ])
        .send()
        .await
        .map_err(|e| format!("Couldn't message banker: {}", e))?;
    Ok(())
}

pub async fn do_query<T: serde::ser::Serialize, U: serde::de::DeserializeOwned>(
    query: &T,
) -> Result<Response<U>, ()> {
    let client = reqwest::Client::new();
    let mut res = client
        .post("https://hn.rishi.cx")
        .json(query)
        .bearer_auth(HN_TOKEN.to_string())
        .send()
        .await
        .map_err(|_| ())?;
    let response_body: Response<U> = res.json().await.map_err(|_| ())?;

    Ok(response_body)
}

pub async fn invoice(user: &str, amount: u64, reason: &str) -> Result<String, String> {
    let query = CreateTransaction::build_query(create_transaction::Variables {
        to: BOT_ID.to_string(),
        from: user.to_string(),
        balance: amount as f64,
        reason: Some(reason.to_string()),
    });

    let result = do_query::<_, create_transaction::ResponseData>(&query)
        .await
        .map_err(|_| String::from("something bad happened"))?
        .data
        .ok_or(String::from("something bad happened"))?;

    Ok(result.transact.id)
}

pub async fn pay(user: String, amount: u64, reason: String) -> Result<(), String> {
    message(format!(
        "<@{}> give <@{}> {} for {}",
        *ID, user, amount, reason
    ))
    .await
    .map_err(|e| format!("Couldn't complete payment: {}", e))
}

pub async fn balance() -> Result<(), String> {
    message(format!("<@{}> balance", *ID))
        .await
        .map_err(|e| format!("Couldn't request balance: {}", e))
}
