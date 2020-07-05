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
    _: Message,
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
                market::log_blocks(
                    format!(
                        "A {} has gone up for sale for {} GP!",
                        possession.name, price
                    )
                    .to_string(),
                    vec![
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
                    ]
                ),
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
                    "Sale failed! Your market fee has been refunded.".to_string(),
                    vec![json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            concat!(
                                "The {} you tried to sell for {}gp is no longer on the market{}"
                            ),
                            name,
                            price,
                            if price >= 20 {
                                format!(", so your {}gp market fee has been refunded.", price/20)
                            } else {
                                ".".to_string()
                            }
                        ))
                    })]
                )
            )
            .map(|_| ()),
        }?;

        banker::balance().await?;

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
    _: Message,
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
        let key = hcor::Key { category, id };
        match hacksteader::get_possession(&db, key).await?.sale {
            Some(sale) => sale,
            None => {
                futures::try_join!(
                    banker::pay(
                        paid_invoice.invoicee.clone(),
                        price,
                        format!("the {} you tried to buy has is no longer on the market", name),
                    ),
                    dm_blocks(
                        paid_invoice.invoicee.clone(),
                        "Sorry, you couldn't purchase that! Your GP has been refunded.".to_string(),
                        vec![json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            concat!(
                                "The {} you tried to buy for {}gp is no longer on the market, ",
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
                table_name: hcor::TABLE_NAME.to_string(),
                ..Default::default()
            }).map_err(|e| format!("database err: {}", e)),
            banker::pay(seller.clone(), price, paid_for),
            market::log_blocks(
                format!("{} purchased a {} on hackmarket for {} GP!", 
                paid_invoice.invoicee,
                name,
                price
                ).to_string(),
                vec![
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
                dm_blocks(
                    seller.clone(),
                    format!("Your sale went through! You earned {} gp.", price).to_string(),
                    vec![
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
    _: Message,
    paid_invoice: banker::PaidInvoice,
) -> HandlerOutput<'a> {
    async move {
        let new_user = paid_invoice.invoicee.clone();
        if !hacksteader::exists(&dyn_db(), new_user.clone()).await {
            Hacksteader::new_in_db(&dyn_db(), new_user.clone())
                .await
                .map_err(|_| "Couldn't put you in the hacksteader database!")?;

            dm_blocks(
                paid_invoice.invoicee.clone(),
                "Welcome to Hackagotchi! Click me for more info.".to_string(),
                vec![
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         "Happy Hacksteading, newcomer! Welcome to Hackagotchi!",
                     )
                 }),
                 json!({ "type": "divider" }),
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         ":house: You can *manage and monitor* your hackstead with the *Home tab*!\n\n\
                         \t_Here you can *keep inventory of the items, plants, and gotchi* you have! \
                         This is also where you *plant seeds*, *hatch eggs*, and *use items*!_"
                     ),
                 }),
                 json!({ "type": "divider" }),
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         ":information_source: \
                         *Use commands* like `/hstead`, `/hstreet`, `/htome`, and `/stateofsteading` \
                         for all the latest in Hackagotchi happenings! \n\n\
                         \t_`/hstead @user` lets you *see a user's hackstead*, \
                         `/hstreet` *opens Hackagotchi's market* to buy items, \
                         `/htome <item name>` gives you basic *information about items*, \
                         and `/stateofsteading` gives you an *overview of the agrarian economy*._"
                     )
                 }),
                 json!({ "type": "divider" }),
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         ":left_speech_bubble: *Join channels* like #hackstead and #hackstreet \
                         to *interact with your fellow Hacksteaders*!\n\n\
                         \t_#hackstead is a great medium for *conversations or suggestions* with other players \
                         and hackagotchi's developers. \
                         #hackstreet provides access to *a live feed of market transactions* \
                         and is a good place to *advertise offers and trades*._"
                     )
                 }),
                 json!({ "type": "divider" }),
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         "We're working on an achievements system to make it easier to get started, \
                         but until then, see if you can complete each of the following:\n\n\
                         * Buy a seed ( :coffea_cyl_seed: / :hacker_vibes_vine_seed: / :bractus_seed: ) from `/hstreet` and plant it!\n\
                         * Craft a compressed resource ( :crystcyl: / :hacksprit: / :bressence: )!\n\
                         * Sacrifice your plant to craft an egg ( :cyl_egg: / :hacker_egg: / :bread_egg: )!\n\
                         * Get a Land Deed :land_deed: from an egg or from a friend to grow two plants at once!\n\
                         * Collect enough Baglings ( :crystalline_buzzwing_bagling: / :spirited_buzzwing_bagling: / :doughy_buzzwing_bagling: ) to get a Megabox ( :crystalline_buzzwing_megabox: / :spirited_buzzwing_megabox: / :doughy_buzzwing_megabox: ), and open it for loot!\n\
                         * Get a :tinkerspore: or an :aloe_avanta_seed: from an egg or from a friend to grow start growing some very rare and overpowered plants!",
                     )
                 }),
                 json!({ "type": "divider" }),
                 json!({
                     "type": "section",
                     "text": mrkdwn(
                         "_For more information, \
                         feel free to contact the Hackagotchi Dev Team with @hackagotchi-dev-team, \
                         visit our website hackagotch.io, \
                         or avail yourself to the <https://hackstead.fandom.com|community run wiki>._"
                     ),
                 }),
                 comment("LET'S HACKSTEAD, FRED!")
            ]).await?;

            let welcome_gifts = CONFIG
                .possession_archetypes
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    p
                        .kind
                        .gotchi()
                        .filter(|g| dbg!(g.welcome_gift))
                        .map(|_| i)
                });
            for ah in welcome_gifts {
                Hacksteader::spawn_possession(&dyn_db(), new_user.clone(), ah)
                    .await
                    .map_err(|e| {
                        let a = format!("couldn't spawn possession: {}", e);
                        error!("{}", e);
                        a
                    })?;
            }
        }
        Ok(())
    }
    .boxed()
}
