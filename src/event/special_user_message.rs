use super::prelude::*;
use super::SpecialUserMessageTrigger;

lazy_static::lazy_static! {
    pub static ref SPAWN_COMMAND: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> spawn (<@([A-z|0-9]+)> )?(([0-9]+) )?(.+)").unwrap(),
        then: &spawn_command
    };
}
fn spawn_command<'a>(
    c: regex::Captures<'a>,
    r: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {
        info!("spawn possession captures: {:?}", c);

        let receiver = c
            .get(3)
            .map(|x| x.as_str())
            .unwrap_or(&r.user_id)
            .to_string();
        let amount = c.get(5).and_then(|x| x.as_str().parse().ok()).unwrap_or(1);
        let possession_name = c
            .get(6)
            .ok_or_else(|| "no item specified".to_string())?
            .as_str();
        let archetype_handle = CONFIG
            .possession_archetypes
            .iter()
            .position(|x| x.name == possession_name)
            .ok_or_else(|| format!("no archetype by name of {}", possession_name))?;

        let arch = CONFIG
            .possession_archetypes
            .get(archetype_handle)
            .expect("invalid archetype handle");

        market::log_blocks(
            format!(
                "<@{}> spawned {} {} for <@{}>!",
                &r.user_id,
                if amount == 1 {
                    "a".to_string()
                } else {
                    amount.to_string()
                },
                arch.name,
                &receiver
            )
            .to_string(),
            vec![
                json!({
                    "type": "section",
                    "text": mrkdwn(format!(
                        concat!(
                            "*{}* new {} *{}* {} been spawned! ",
                            "Special user <@{}> spawned {} for <@{}>.",
                        ),
                        amount,
                        emojify(&arch.name),
                        arch.name,
                        if amount == 1 {
                            "has"
                        } else {
                            "have"
                        },
                        &r.user_id,
                        if amount == 1 {
                            "it"
                        } else {
                            "them"
                        },
                        &receiver,
                    )),
                    "accessory": {
                        "type": "image",
                        "image_url": format!(
                            "http://{}/gotchi/img/{}/{}.png",
                            *URL,
                            format!("{:?}", arch.kind.category()).to_lowercase(),
                            filify(&arch.name)
                        ),
                        "alt_text": "Hackpheus holding a Gift!",
                    }
                }),
                comment("U GET AN EGG, U GET AN EGG, U GET AN EGG!"),
            ],
        )
        .await?;

        // todo: async concurrency
        for _ in 0_usize..amount {
            Hacksteader::spawn_possession(&dyn_db(), receiver.clone(), archetype_handle)
                .await
                .map_err(|e| {
                    let a = format!("couldn't spawn possession: {}", e);
                    error!("{}", e);
                    a
                })?;
        }
        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref GP_DUMP_COMMAND: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> dump <@([A-z|0-9]+)> ([0-9]+)").unwrap(),
        then: &gp_dump_command
    };
}
fn gp_dump_command<'a>(
    c: regex::Captures<'a>,
    _: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {
        let dump_to = c
            .get(2)
            .ok_or_else(|| "no dump receiver".to_string())?
            .as_str()
            .to_string();
        let dump_amount = c
            .get(3)
            .ok_or_else(|| "no dump amount".to_string())?
            .as_str()
            .parse::<u64>()
            .map_err(|e| format!("invalid dump amount: {}", e))?;

        info!("dumping {} to {}", dump_amount, dump_to);
        banker::pay(dump_to, dump_amount, "GP dump".to_string()).await?;
        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref YANK_CONFIG: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> goblin chant").unwrap(),
        then: &yank_config
    };
}
fn yank_config<'a>(
    _: regex::Captures<'a>,
    _: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    const CHANTING_DESCRIPTIONS: &'static [&'static str] = &[
        "eerie",
        "eldritch",
        "rhythmic",
        "spooky",
        "tribal",
        "unholy",
        "deafening",
        "savage",
        "canadian",
    ];
    use rand::seq::SliceRandom;

    async move {
        banker::message(match crate::yank_config::yank_config().await {
            Ok(()) => format!(
                "{} goblin chanting hath brought forth new config from the heavens!",
                CHANTING_DESCRIPTIONS
                    .choose(&mut rand::thread_rng())
                    .unwrap()
            ),
            Err(e) => format!("goblin chanting interrupted by vile belch:\n{}", e),
        })
        .await
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref STOMP_COMMAND: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> goblin stomp").unwrap(),
        then: &stomp_command
    };
}
fn stomp_command<'a>(
    _: regex::Captures<'a>,
    _: Message,
    to_farming: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {
        info!("goblin_stomp time!");

        match hacksteader::goblin_stomp(&dyn_db(), &to_farming).await {
            Ok(()) => {}
            Err(e) => error!("goblin stomp error: {}", e),
        }
        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref SLAUGHTER_COMMAND: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> goblin slaughter").unwrap(),
        then: &slaughter_command
    };
}
fn slaughter_command<'a>(
    _: regex::Captures<'a>,
    _: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {
        info!("goblin slaughter time!");

        match hacksteader::goblin_slaughter(&dyn_db()).await {
            Ok(()) => {}
            Err(e) => error!("goblin slaughter error: {}", e),
        };
        Ok(())
    }
    .boxed()
}

lazy_static::lazy_static! {
    pub static ref NAB_COMMAND: SpecialUserMessageTrigger = SpecialUserMessageTrigger {
        regex: Regex::new("<@([A-z|0-9]+)> goblin nab (.+)").unwrap(),
        then: &nab_command
    };
}
fn nab_command<'a>(
    c: regex::Captures<'a>,
    _: Message,
    _: &'a Sender<FarmingInputEvent>,
) -> HandlerOutput<'a> {
    async move {
        info!("goblin nab time!");

        let archetype_handle = CONFIG
            .find_possession_handle(
                &c.get(2)
                    .ok_or_else(|| "no item to nab".to_string())?
                    .as_str(),
            )
            .map_err(|e| format!("unknown item: {}", e))?;

        let db = &dyn_db();

        let scan = db.scan(rusoto_dynamodb::ScanInput {
            table_name: hcor::TABLE_NAME.to_string(),
            filter_expression: Some("cat = :item_cat AND archetype_handle = :ah".to_string()),
            expression_attribute_values: Some(
                [
                    (":item_cat".to_string(), Category::Misc.into_av()),
                    (
                        ":ah".to_string(),
                        rusoto_dynamodb::AttributeValue {
                            n: Some(archetype_handle.to_string()),
                            ..Default::default()
                        },
                    ),
                ]
                .iter()
                .cloned()
                .collect(),
            ),
            ..Default::default()
        });

        match scan.await {
            Ok(rusoto_dynamodb::ScanOutput {
                items: Some(items), ..
            }) => {
                stream::iter(
                    items
                        .into_iter()
                        .map(|mut i| rusoto_dynamodb::WriteRequest {
                            delete_request: Some(rusoto_dynamodb::DeleteRequest {
                                key: [
                                    ("cat".to_string(), i.remove("cat").unwrap()),
                                    ("id".to_string(), i.remove("id").unwrap()),
                                ]
                                .iter()
                                .cloned()
                                .collect(),
                            }),
                            ..Default::default()
                        })
                        .collect::<Vec<_>>()
                        .chunks(25)
                        .map(|items| {
                            db.batch_write_item(rusoto_dynamodb::BatchWriteItemInput {
                                request_items: [(hcor::TABLE_NAME.to_string(), items.to_vec())]
                                    .iter()
                                    .cloned()
                                    .collect(),
                                ..Default::default()
                            })
                        }),
                )
                .map(|x| Ok(x))
                .try_for_each_concurrent(None, |r| async move {
                    match r.await {
                        Ok(_) => Ok(()),
                        Err(e) => Err(format!("error deleting for goblin nab: {}", e)),
                    }
                })
                .await
                .map_err(|e| {
                    let a = format!("goblin nab async err: {}", e);
                    error!("{}", a);
                    a
                })?;
            }
            Err(e) => error!("goblin nab error: {}", e),
            _ => error!("scan returned no items for nab request!"),
        }
        Ok(())
    }
    .boxed()
}
