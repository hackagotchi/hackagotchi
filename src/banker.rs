use crate::{dm_blocks, event::Message, mrkdwn, ID as BOT_ID, TOKEN};
use log::{debug, info};
use regex::Regex;

use graphql_client::{GraphQLQuery, QueryBody, Response};
use serde::de::DeserializeOwned;
use serde_json::json;

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

#[derive(GraphQLQuery)]
#[graphql(schema_path = "hn/schema.json", query_path = "hn/pay.graphql")]
pub struct Pay;

#[derive(GraphQLQuery)]
#[graphql(schema_path = "hn/schema.json", query_path = "hn/get_balance.graphql")]
pub struct GetBalance;

use std::env::var;
lazy_static::lazy_static! {
    pub static ref CHAT_ID: String = var("BANKER_CHAT").unwrap();
    pub static ref HN_TOKEN: String = var("HN_TOKEN").unwrap();
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
        .header("secret", HN_TOKEN.to_string())
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

    dm_blocks(user.to_string(), format!("I've just invoiced you {} HN for \"{}\". Type `/pay {}` in the chat below to confirm.", amount, reason, result.transact.id), vec![
        json!({
            "type": "section",
            "text": mrkdwn(format!("I've just invoiced you {} HN for \"{}\". Type `/pay {}` in the chat below to confirm.", amount, reason, result.transact.id))
        })
    ]).await?;

    Ok(result.transact.id)
}

pub async fn pay(user: String, amount: u64, reason: String) -> Result<(), String> {
    let query = Pay::build_query(pay::Variables {
        to: user.clone(),
        from: BOT_ID.to_string(),
        amount: amount.clone() as f64,
        reason: Some(reason.clone()),
    });

    let result = do_query::<_, pay::ResponseData>(&query)
        .await
        .expect("something bad happened")
        .data
        .expect("something bad happened");

    dm_blocks(
        user.to_string(),
        format!("I've just sent you {} HN for \"{}\"!", amount, reason),
        vec![json!({
            "type": "section",
            "text": mrkdwn(format!("I've just sent you {} HN for \"{}\"!", amount, reason))
        })],
    )
    .await?;

    Ok(())
}

pub async fn get_balance() -> Result<u64, String> {
    let query = GetBalance::build_query(get_balance::Variables {
        user: BOT_ID.to_string(),
    });

    let result = do_query::<_, get_balance::ResponseData>(&query)
        .await
        .expect("something bad happened")
        .data
        .expect("something bad happened");

    Ok(result.user.balance as u64)
}
