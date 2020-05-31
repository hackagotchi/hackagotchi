use super::prelude::*;
use super::InvoicePaymentTrigger;

pub struct Sale {
    name: String,
    price: u64,
    id: uuid::Uuid,
    category: Category,
    from: Option<String>,
}
impl Sale {
    fn from_captures(captures: &regex::Captures) -> Result<Self, String> {
        Ok(Sale {
            name: captures
                .get(1)
                .ok_or_else(|| "no name in sale".to_string())?
                .as_str()
                .to_string(),
            price: captures
                .get(2)
                .ok_or_else(|| "no price in sale".to_string())?
                .as_str()
                .parse()
                .map_err(|e| format!("sale price number parsing: {}", e))?,
            id: uuid::Uuid::parse_str(
                captures
                    .get(3)
                    .ok_or_else(|| "no id in sale".to_string())?
                    .as_str(),
            )
            .map_err(|e| format!("invalid uuid in sale: {}", e))?,
            category: captures
                .get(4)
                .ok_or_else(|| "no category in sale".to_string())?
                .as_str()
                .parse::<u8>()
                .map_err(|e| format!("err parsing id number in sale: {}", e))?
                .try_into()
                .map_err(|e| format!("couldn't turn category number into id in sale: {}", e))?,
            from: captures.get(5).map(|x| x.as_str().to_string()),
        })
    }
}

lazy_static::lazy_static! {
    pub static ref HACKMARKET_FEES: InvoicePaymentTrigger = InvoicePaymentTrigger {
        regex: Regex::new("hackmarket fees for selling (.+) at ([0-9]+)gp :(.+):([0-9])").unwrap(),
        then: &hackmarket_fees
    };
}
fn hackmarket_fees<'a>(
    c: regex::Captures<'a>,
    _: Message<'a>,
    paid_invoice: banker::PaidInvoice,
) -> HandlerOutput<'a> {
    async move {
        let Sale {
            name,
            price,
            id,
            category,
            ..
        } = Sale::from_captures(&c)?;

        let db = dyn_db();
        let key = Key { category, id };
        let possession = hacksteader::get_possession(&db, key).await?;
        match possession.sale {
            None => futures::try_join!(
                market::place_on_market(&db, key, price, name.clone()),
                market::log_blocks(vec![
                    json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "A *{}* has gone up for sale! \
                            <@{}> is selling it on the hackmarket for *{} GP*!",
                            possession.name, paid_invoice.invoicee, price
                        )),
                        "accessory": {
                            "type": "image",
                            "image_url": format!(
                                "http://{}/gotchi/img/{}/{}.png",
                                *URL,
                                category,
                                filify(&possession.name)
                            ),
                            "alt_text": "Hackpheus sitting on bags of money!",
                        }
                    }),
                    comment("QWIK U BETTR BYE ET B4 SUM1 EYLS"),
                ]),
            )
            .map(|_| ()),
            Some(_) => futures::try_join!(
                banker::pay(
                    possession.steader.clone(),
                    price / 20,
                    format!("the {} you tried to sell is already up for sale", name),
                ),
                dm_blocks(
                    paid_invoice.invoicee.clone(),
                    vec![json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            concat!(
                                "The {} you tried to sell for {}gp has already been sold, ",
                                "so your {}gp market fee has been refunded."
                            ),
                            name,
                            price,
                            price / 20
                        ))
                    })]
                )
            )
            .map(|_| ()),
        }?;

        //banker::balance().await?;

        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref HACKMARKET_PURCHASE: InvoicePaymentTrigger = InvoicePaymentTrigger {
        regex: Regex::new("hackmarket purchase buying (.+) at ([0-9]+)gp :(.+):([0-9]) from <@([A-z|0-9]+)>").unwrap(),
        then: &hackmarket_purchase
    };
}
fn hackmarket_purchase<'a>(
    c: regex::Captures<'a>,
    _: Message<'a>,
    paid_invoice: banker::PaidInvoice,
) -> HandlerOutput<'a> {
    async move {
        let Sale {
            name,
            price,
            from,
            id,
            category,
            ..
        } = Sale::from_captures(&c)?;
        let seller = from.ok_or_else(|| "no seller in sale object parsed from invoice reason".to_string())?;

        let db = dyn_db();
        let key = core::Key { category, id };
        match hacksteader::get_possession(&db, key).await?.sale {
            Some(sale) => sale,
            None => {
                futures::try_join!(
                    banker::pay(
                        paid_invoice.invoicee.clone(),
                        price,
                        format!("the {} you tried to buy has already been sold", name),
                    ),
                    dm_blocks(paid_invoice.invoicee.clone(), vec![json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            concat!(
                                "The {} you tried to buy for {}gp has already been sold, ",
                                "so your GP has been refunded."
                            ),
                            name,
                            price
                        ))
                    })])
                )?;
                return Ok(());
            }
        };

        let paid_for = format!("sale of your {}", name);
        futures::try_join!(
            db.update_item(rusoto_dynamodb::UpdateItemInput {
                key: [
                    ("cat".to_string(), category.into_av()),
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
                expression_attribute_values: Some([
                    (":new_owner".to_string(), AttributeValue {
                        s: Some(paid_invoice.invoicee.clone()),
                        ..Default::default()
                    }),
                    (":ownership_entry".to_string(), AttributeValue {
                        l: Some(vec![possess::Owner {
                            id: paid_invoice.invoicee.clone(),
                            acquisition: possess::Acquisition::Purchase {
                                price: paid_invoice.amount,
                            }
                        }.into()]),
                        ..Default::default()
                    })
                ].iter().cloned().collect()),
                update_expression: Some(concat!(
                    "REMOVE price, market_name ",
                    "SET steader = :new_owner, ownership_log = list_append(ownership_log, :ownership_entry)"
                ).to_string()),
                table_name: core::TABLE_NAME.to_string(),
                ..Default::default()
            }).map_err(|e| format!("database err: {}", e)),
            banker::pay(seller.clone(), price, paid_for),
            market::log_blocks(vec![
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "The sale of a *{}* has gone through! \
                        <@{}> made the purchase on hackmarket, earning <@{}> *{} GP*!",
                        name, paid_invoice.invoicee, seller, price
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!("http://{}/gotchi/img/{}/{}.png", *URL, category, filify(&name)),
                        "alt_text": "Hackpheus sitting on bags of money!",
                    }
                }),
                comment("U NO GET 2 BYE DAT 1"),
            ]),
            dm_blocks(seller.clone(), vec![
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        "The sale of your *{}* has gone through! \
                        <@{}> made the purchase on hackmarket, earning you *{} GP*!",
                        name, paid_invoice.invoicee, price
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!("http://{}/gotchi/img/{}/{}.png", *URL, category, filify(&name)),
                        "alt_text": "Hackpheus sitting on bags of money!",
                    }
                }),
                comment("BRUH UR LIKE ROLLING IN CASH"),
            ])
        )
        .map_err(|e| {
            let a = format!("Couldn't complete sale of {}: {}", id, e);
            error!("{}", a);
            a
        })?;

        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref START_HACKSTEAD_INVOICE_PAYMENT: InvoicePaymentTrigger = InvoicePaymentTrigger {
        regex: Regex::new("let's hackstead, fred!").unwrap(),
        then: &start_hackstead_invoice_payment
    };
}
fn start_hackstead_invoice_payment<'a>(
    _: regex::Captures<'a>,
    _: Message<'a>,
    paid_invoice: banker::PaidInvoice,
) -> HandlerOutput<'a> {
    async move {
        if !hacksteader::exists(&dyn_db(), paid_invoice.invoicee.clone()).await {
            Hacksteader::new_in_db(&dyn_db(), paid_invoice.invoicee.clone())
                .await
                .map_err(|_| "Couldn't put you in the hacksteader database!")?;

            dm_blocks(paid_invoice.invoicee.clone(), vec![
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         "Congratulations, new Hacksteader! \
                         Welcome to the community!\n\n\
                         :house: Manage your hackstead in the home tab above\n\
                         :harder-flex: Show anyone a snapshot of your hackstead with /hackstead\n\
                         :sleuth_or_spy: Look up anyone else's hackstead with /hackstead @<their name>\n\
                         :money_with_wings: Shop from the user-run hacksteaders' market with /hackmarket\n\n\
                         You might want to start by buying some seeds there and planting them at your hackstead!"
                     ),
                 }),
                 comment("LET'S HACKSTEAD, FRED!")
            ]).await?;
        }
        Ok(())
    }
    .boxed()
}
