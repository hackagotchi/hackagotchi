use log::{error, info};
use rocket::post;

use rocket_contrib::json::Json;
use serde::Deserialize;

use graphql_client::GraphQLQuery;

use crate::banker;
use crate::banker::do_query;

use crate::event::InvoicePaymentTrigger;
use crate::event::INVOICE_PAYMENT_TRIGGERS;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "hn/schema.json",
    query_path = "hn/get_transaction.graphql"
)]
pub struct GetTransaction;

#[derive(Deserialize, Debug)]
pub enum HnWebhookType {
    #[serde(rename = "payment")]
    Payment,

    #[serde(rename = "transaction")]
    Transaction,
}
#[derive(Deserialize, Debug)]
pub struct HnWebhook {
    body: HnWebhookBody,
    timeout: u32,
}

#[derive(Deserialize, Debug)]
pub struct HnWebhookBody {
    #[serde(rename = "type")]
    webhook_type: HnWebhookType,

    id: String,
}

#[post("/hn/transaction", data = "<webhook>")]
pub async fn transaction(webhook: Json<HnWebhook>) {
    // To possibly be used later
}

#[post("/hn/payment", data = "<webhook>")]
pub async fn payment(webhook: Json<HnWebhook>) {
    let query = GetTransaction::build_query(get_transaction::Variables {
        id: webhook.body.id.clone(),
    });

    let resp = do_query::<_, get_transaction::ResponseData>(&query)
        .await
        .expect("getting transaction failed")
        .data
        .expect("getting transaction failed");

    info!("invoice {} just paid", webhook.body.id);

    for InvoicePaymentTrigger { regex, then } in INVOICE_PAYMENT_TRIGGERS.iter() {
        let c = match regex.captures(&resp.transaction.for_) {
            Some(c) => c,
            None => continue,
        };
        if let Err(e) = then(
            c,
            banker::PaidInvoice {
                invoicer: resp.transaction.to.id.clone(),
                invoicee: resp.transaction.from.id.clone(),
                reason: resp.transaction.for_.clone(),
                amount: resp.transaction.balance.clone() as u64,
            },
        )
        .await
        {
            banker::message(format!("invoice payment handler err : {}", e))
                .await
                .unwrap_or_else(|e| error!("{}", e));
        }
    }
}
