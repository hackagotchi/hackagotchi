use super::{Reply, TOKEN};
use regex::Regex;
use std::env::var;

#[derive(Debug)]
pub struct PaidInvoice {
    pub invoicer: String,
    pub amount: usize,
    pub invoicee: String,
}

lazy_static::lazy_static! {
    pub static ref ID: String = var("BANKER_ID").unwrap();
    static ref CHAT_ID: String = var("BANKER_CHAT").unwrap();
    static ref PAID_INVOICE_MSG_REGEX: Regex = Regex::new(
        "<@([A-z|0-9]+)> paid their invoice of ([0-9]+) gp from <@([A-z|0-9]+)>."
    ).unwrap();
}

/// Takes a threaded reply, returns `Some` if that threaded reply contains
/// an invoice payment confirmation message
pub fn parse_paid_invoice(msg: &Reply) -> Option<PaidInvoice> {
    if msg.channel == *CHAT_ID && msg.user_id == *ID {
        let caps = dbg!(PAID_INVOICE_MSG_REGEX.captures(&msg.text))?;
        return Some(PaidInvoice {
            invoicee: caps.get(1)?.as_str().to_string(),
            amount: caps.get(2)?.as_str().parse().ok()?,
            invoicer: caps.get(3)?.as_str().to_string(),
        });
    }
    None
}

pub async fn message(msg: &str) -> reqwest::Result<()> {
    let client = reqwest::Client::new();
    client
        .post("https://slack.com/api/chat.postMessage")
        .form(&[
            ("token", TOKEN.as_str()),
            ("channel", CHAT_ID.as_str()),
            ("text", msg),
        ])
        .send()
        .await?;
    Ok(())
}
