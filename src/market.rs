use super::hacksteader;
use hacksteader::{Category, Sk};
use rusoto_dynamodb::{AttributeValue, DynamoDb, DynamoDbClient};
use std::collections::HashMap;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Sale {
    pub id: uuid::Uuid,
    pub price: u64,
    pub seller: String,
    pub name: String,
    pub category: Category,
}
impl Sale {
    pub fn sk(&self) -> String {
        let Self {
            id, seller, name, ..
        } = self.clone();
        Sk { id, owner: seller, name }.to_string()
    }
}

pub async fn market_search(db: &DynamoDbClient) -> Option<Vec<Sale>> {
    let query = db
        .query(rusoto_dynamodb::QueryInput {
            table_name: hacksteader::TABLE_NAME.to_string(),
            key_condition_expression: Some("begins_with(id, :order_marker)".to_string()),
            expression_attribute_values: Some({
                let mut m = HashMap::new();
                m.insert(
                    ":order_marker".to_string(),
                    AttributeValue {
                        s: Some("O".to_string()),
                        ..Default::default()
                    },
                );
                m
            }),
            ..Default::default()
        })
        .await;

    Some(
        query
            .ok()?
            .items?
            .iter_mut()
            .filter_map(|i| {
                let Sk { name, id, owner: seller } = Sk::from_string(i.remove("sk")?.s.as_ref()?)?;
                Some(Sale {
                    id,
                    name,
                    seller,
                    category: Category::from_av(i.get("cat")?)?,
                    price: i.remove("price")?.n?.parse().ok()?,
                })
            })
            .collect(),
    )
}

pub async fn place_on_market(
    db: &DynamoDbClient,
    seller_id: String,
    possession_id: String,
    price: u64,
) -> Result<(), String> {
    db.put_item(rusoto_dynamodb::PutItemInput {
        item: [
            (
                "id".to_string(),
                AttributeValue {
                    s: Some("O".to_string()),
                    ..Default::default()
                },
            ),
            (
                "sk".to_string(),
                AttributeValue {
                    s: Some(possession_id.clone()),
                    ..Default::default()
                },
            ),
            (
                "seller".to_string(),
                AttributeValue {
                    s: Some(seller_id),
                    ..Default::default()
                },
            ),
            (
                "price".to_string(),
                AttributeValue {
                    n: Some(price.to_string()),
                    ..Default::default()
                },
            ),
        ]
        .iter()
        .cloned()
        .collect(),
        table_name: hacksteader::TABLE_NAME.to_string(),
        ..Default::default()
    })
    .await
    .map_err(|e| format!("Couldn't place {} on market: {}", possession_id, e))?;

    Ok(())
}
