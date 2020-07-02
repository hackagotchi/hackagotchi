use super::prelude::*;
use super::BankerMessageTrigger;

lazy_static::lazy_static! {
    pub static ref BANKER_BALANCE: BankerMessageTrigger = BankerMessageTrigger {
        regex: Regex::new("You have ([0-9]+)gp in your account, hackalacker.").unwrap(),
        then: &banker_balance
    };
}
fn banker_balance<'a>(
    c: regex::Captures<'a>,
    _: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {

        let balance = c
            .get(1)
            .ok_or_else(|| "no gp amount".to_string())?
            .as_str()
            .parse::<u64>()
            .map_err(|e| format!("error parsing number in banker balance msg: {}", e))?;
        info!("I got {} problems and GP ain't one", balance);

        let query = dyn_db()
            .query(rusoto_dynamodb::QueryInput {
                table_name: hcor::TABLE_NAME.to_string(),
                key_condition_expression: Some("cat = :gotchi_cat".to_string()),
                expression_attribute_values: Some({
                    [(":gotchi_cat".to_string(), Category::Gotchi.into_av())]
                        .iter()
                        .cloned()
                        .collect()
                }),
                ..Default::default()
            })
            .await;

        let gotchis = query
            .map_err(|e| format!("couldn't query all gotchis: {}", e))?
            .items
            .ok_or("no gotchis found!")?
            .iter()
            .filter_map(|i| match Possession::from_item(i) {
                Ok(p) => Some(Possessed::<Gotchi>::from_possession(p).unwrap()),
                Err(e) => {
                    error!("error parsing gotchi: {}", e);
                    None
                }
            })
            .filter(|g| g.inner.base_happiness > 0)
            .collect::<Vec<Possessed<Gotchi>>>();

        let total_happiness: u64 = gotchis.iter().map(|x| x.inner.base_happiness).sum();
        let mut funds_awarded = 0;

        for _ in 0..balance / total_happiness {
            stream::iter(gotchis.clone()).map(|x| Ok(x)).try_for_each_concurrent(None, |gotchi| {
                let dm = vec![
                    json!({
                        "type": "section",
                        "text": mrkdwn(format!(
                            "_It's free GP time, ladies and gentlegotchis!_\n\n\
                            It seems your lovely Gotchi *{}* has collected *{} GP* for you!",
                            gotchi.inner.nickname,
                            gotchi.inner.base_happiness
                        )),
                        "accessory": {
                            "type": "image",
                            "image_url": format!("http://{}/gotchi/img/{}/{}.png", *URL, Category::Gotchi, filify(&gotchi.name)),
                            "alt_text": "Hackpheus holding a Gift!",
                        }
                    }),
                    comment("IN HACK WE STEAD")
                ];
                let payment_note = format!(
                    "{} collected {} GP for you",
                    gotchi.inner.nickname,
                    gotchi.inner.base_happiness,
                );
                let steader_in_harvest_log: bool = gotchi.inner.harvest_log.last().filter(|x| x.id == gotchi.steader).is_some();
                let db_update = rusoto_dynamodb::UpdateItemInput {
                    table_name: hcor::TABLE_NAME.to_string(),
                    key: gotchi.clone().into_possession().key().into_item(),
                    update_expression: Some(if steader_in_harvest_log {
                        format!("ADD harvest_log[{}].harvested :harv", gotchi.inner.harvest_log.len() - 1)
                    } else {
                        "SET harvest_log = list_append(harvest_log, :harv)".to_string()
                    }),
                    expression_attribute_values: Some(
                        [(
                            ":harv".to_string(),
                            if steader_in_harvest_log {
                                AttributeValue {
                                    n: Some(gotchi.inner.base_happiness.to_string()),
                                    ..Default::default()
                                }
                            } else {
                                AttributeValue {
                                    l: Some(vec![possess::gotchi::GotchiHarvestOwner {
                                        id: gotchi.steader.clone(),
                                        harvested: gotchi.inner.base_happiness,
                                    }.into()]),
                                    ..Default::default()
                                }
                            }
                        )]
                        .iter()
                        .cloned()
                        .collect(),
                    ),
                    ..Default::default()
                };
                let db = dyn_db();

                async move {
                    futures::try_join!(
                        dm_blocks(gotchi.steader.clone(), "It's GP Time! Your Gotchi produced some GP for you...".to_string(), dm),
                        banker::pay(gotchi.steader.clone(), gotchi.inner.base_happiness, payment_note),
                        db.update_item(db_update).map_err(|e| format!("Couldn't update owner log: {}", e))
                    )?;
                    Ok(())
                }
            })
            .await
            .map_err(|e: String| format!("harvest msg send err: {}", e))?;

            funds_awarded += total_happiness;
        }

        futures::try_join!(
            banker::message(format!("{} GP earned this harvest!", funds_awarded)),
            banker::message(format!("total happiness: {}", total_happiness)),
        )?;
        Ok(())
    }
    .boxed()
}
