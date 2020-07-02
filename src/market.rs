use hcor::{market::Sale, Category, Key, Possession};
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};

use std::env::var;
lazy_static::lazy_static! {

    pub static ref HACKMARKET_LOG_CHAT: String = var("HACKMARKET_LOG_CHAT").unwrap();
}

pub async fn log_blocks(notif_msg: String, blocks: Vec<serde_json::Value>) -> Result<(), String> {
    let o = serde_json::json!({
        "channel": *HACKMARKET_LOG_CHAT,
        "token": *super::TOKEN,
        "blocks": blocks,
        "text": notif_msg
    });

    log::debug!("{}", serde_json::to_string_pretty(&o).unwrap());

    // TODO: use response
    let client = reqwest::Client::new();
    client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(&*super::TOKEN)
        .json(&o)
        .send()
        .await
        .map_err(|e| format!("couldn't log blocks: {}", e))?;

    Ok(())
}

pub async fn market_search(
    db: &DynamoDbClient,
    cat: Category,
) -> Result<Vec<(Sale, Possession)>, String> {
    let query = db
        .query(rusoto_dynamodb::QueryInput {
            table_name: hcor::TABLE_NAME.to_string(),
            index_name: Some("cat_price_index".to_string()),
            key_condition_expression: Some("cat = :sale_cat".to_string()),
            expression_attribute_values: Some(
                [(":sale_cat".to_string(), cat.into_av())]
                    .iter()
                    .cloned()
                    .collect(),
            ),
            ..Default::default()
        })
        .await;

    Ok(query
        .map_err(|e| dbg!(format!("Couldn't search market: {}", e)))?
        .items
        .ok_or_else(|| format!("market search query returned no items"))?
        .iter_mut()
        .filter_map(|i| match Possession::from_item(i) {
            Ok(mut pos) => Some((pos.sale.take()?, pos)),
            Err(e) => {
                println!("error parsing possession: {}", e);
                None
            }
        })
        .collect())
}

pub async fn place_on_market(
    db: &DynamoDbClient,
    key: Key,
    price: u64,
    name: String,
) -> Result<(), String> {
    println!("putting {} on the market", key.id);

    db.update_item(rusoto_dynamodb::UpdateItemInput {
        key: key.clone().into_item(),
        expression_attribute_values: Some(
            [
                (
                    ":sale_price".to_string(),
                    AttributeValue {
                        n: Some(price.to_string()),
                        ..Default::default()
                    },
                ),
                (
                    ":new_name".to_string(),
                    AttributeValue {
                        s: Some(name),
                        ..Default::default()
                    },
                ),
            ]
            .iter()
            .cloned()
            .collect(),
        ),
        update_expression: Some("SET price = :sale_price, market_name = :new_name".to_string()),
        table_name: hcor::TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| dbg!(format!("Couldn't place {} on market: {}", key.id, e)))?;

    Ok(())
}

pub async fn take_off_market(db: &DynamoDbClient, key: Key) -> Result<(), String> {
    println!("taking {} off the market", key.id);

    db.update_item(rusoto_dynamodb::UpdateItemInput {
        key: key.clone().into_item(),
        update_expression: Some("REMOVE price, market_name".to_string()),
        table_name: hcor::TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| dbg!(format!("Couldn't remove {} from market: {}", key.id, e)))?;

    Ok(())
}
