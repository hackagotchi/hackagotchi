
fn market_search() -> Vec<_> {
    let query = db
        .query(rusoto_dynamodb::QueryInput {
            table_name: TABLE_NAME.to_string(),
            key_condition_expression: Some("id = :db_id".to_string()),
            expression_attribute_values: Some({
                let mut m = HashMap::new();
                m.insert(
                    ":db_id".to_string(),
                    AttributeValue {
                        s: Some(user_id.clone()),
                        ..Default::default()
                    },
                    );
                m
            }),
            ..Default::default()
        })
    .await;
    let items = query.ok()?.items?;
}
