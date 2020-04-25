use super::{Message, TOKEN};
use regex::Regex;
use std::env::var;

#[derive(Debug)]
pub struct PaidInvoice {
    pub invoicer: String,
    pub amount: u64,
    pub invoicee: String,
    pub reason: String,
}

lazy_static::lazy_static! {
    pub static ref ID: String = var("BANKER_ID").unwrap();
    pub static ref CHAT_ID: String = var("BANKER_CHAT").unwrap();
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

pub async fn invoice(user: &str, amount: u64, reason: &str) -> Result<(), String> {
    message(format!(
        "<@{}> invoice <@{}> {} for {}",
        *ID, user, amount, reason
    ))
    .await
    .map_err(|e| format!("Couldn't request invoice: {}", e))
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
