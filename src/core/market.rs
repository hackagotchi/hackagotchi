use crate::AttributeParseError;

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Sale {
    pub price: u64,
    pub market_name: String,
}
impl Sale {
    pub fn from_item(i: &crate::Item) -> Result<Self, crate::AttributeParseError> {
        use AttributeParseError::*;

        Ok(Sale {
            market_name: i
                .get("market_name")
                .ok_or(MissingField("market_name"))?
                .s
                .as_ref()
                .ok_or(WronglyTypedField("market_name"))?
                .clone(),
            price: i.get("price")?.n.as_ref()?.parse().ok()?,
        })
    }
}
