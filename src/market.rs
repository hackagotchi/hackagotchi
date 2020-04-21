use super::hacksteader;
use hacksteader::{Category, Possessed, Gotchi};
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Sale {
    pub price: u64,
    pub market_name: String,
}

pub async fn market_search(db: &DynamoDbClient, cat: Category) -> Option<Vec<(Possessed<Gotchi>, Sale)>> {
    let query = db
        .query(rusoto_dynamodb::QueryInput {
            table_name: hacksteader::TABLE_NAME.to_string(),
            index_name: Some("category_price_index".to_string()),
            key_condition_expression: Some("cat = :sale_cat".to_string()),
            expression_attribute_values: Some(
                [(":sale_cat".to_string(), cat.into_av())]
                    .iter()
                    .cloned()
                    .collect()
            ),
            ..Default::default()
        })
        .await;

    Some(
        dbg!(query
            .map_err(|e| dbg!(format!("Couldn't search market: {}", e)))
            .ok()?
            .items)?
            .iter_mut()
            .filter_map(|i| Some((Possessed::from_item(i)?, Sale {
                market_name: i.remove("market_name")?.s?,
                price: i.remove("price")?.n?.parse().ok()?
            })))
            .collect(),
    )
}

pub async fn place_on_market(
    db: &DynamoDbClient,
    cat: Category,
    id: uuid::Uuid,
    price: u64,
    name: String,
) -> Result<(), String> {
    println!("putting {} on the market", id);

    db.update_item(rusoto_dynamodb::UpdateItemInput {
        key: [
            ("cat".to_string(), cat.into_av()),
            (
                "id".to_string(),
                AttributeValue {
                    s: Some(id.to_string()),
                    ..Default::default()
                },
            ),
        ]
        .iter()
        .cloned()
        .collect(),
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
                    }
                )
            ]
            .iter()
            .cloned()
            .collect(),
        ),
        update_expression: Some("SET price = :sale_price, market_name = :new_name".to_string()),
        table_name: hacksteader::TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| dbg!(format!("Couldn't place {} on market: {}", id, e)))?;

    Ok(())
}

pub async fn take_off_market(
    db: &DynamoDbClient,
    cat: Category,
    id: uuid::Uuid,
) -> Result<(), String> {
    println!("taking {} off the market", id);

    db.update_item(rusoto_dynamodb::UpdateItemInput {
        key: [
            ("cat".to_string(), cat.into_av()),
            (
                "id".to_string(),
                AttributeValue {
                    s: Some(id.to_string()),
                    ..Default::default()
                },
            ),
        ]
        .iter()
        .cloned()
        .collect(),
        update_expression: Some("REMOVE price, market_name".to_string()),
        table_name: hacksteader::TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| dbg!(format!("Couldn't remove {} from market: {}", id, e)))?;

    Ok(())
}
