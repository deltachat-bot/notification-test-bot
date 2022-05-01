use async_std::channel;
use chrono::{DateTime, Utc};
use deltachat::contact::ContactId;
use futures_lite::future::FutureExt;
use rand::Rng;
use std::env::{current_dir, vars};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use deltachat::chat::*;
use deltachat::config;
use deltachat::constants::Chattype;
use deltachat::context::*;
use deltachat::message::*;
use deltachat::EventType;

async fn handle_message(
    ctx: &Context,
    chat_id: ChatId,
    msg_id: MsgId,
    database: &sled::Db,
) -> Result<(), anyhow::Error> {
    let mut chat = Chat::load_from_db(ctx, chat_id).await?;
    if chat.is_contact_request() {
        chat_id.accept(ctx).await?;
        chat = Chat::load_from_db(ctx, chat_id).await?;
    }

    let msg = Message::load_from_db(ctx, msg_id).await?;
    if msg.get_from_id() == ContactId::SELF {
        // prevent loop (don't react to own messages)
        return Ok(());
    }

    println!(
        "recieved message '{}' in chat with type {:?}",
        msg.get_text().unwrap_or_else(|| "".to_owned()),
        chat.get_type()
    );

    if chat.get_type() == Chattype::Single {
        match msg.get_text().context("message text")?.as_ref() {
            "/subscribe" | "/sub" => {
                if database
                    .insert(msg.get_from_id().to_u32().to_string(), "true")
                    .is_ok()
                {
                    let mut message = Message::new(Viewtype::Text);
                    message.set_text(Some("subscribed".to_owned()));
                    send_msg(ctx, chat_id, &mut message).await?;
                } else {
                    let mut message = Message::new(Viewtype::Text);
                    message.set_text(Some("subscription failed".to_owned()));
                    send_msg(ctx, chat_id, &mut message).await?;
                }
            }
            "/unsubscribe" | "/out" => {
                if database
                    .remove(msg.get_from_id().to_u32().to_string())
                    .is_ok()
                {
                    let mut message = Message::new(Viewtype::Text);
                    message.set_text(Some("unsubscribed".to_owned()));
                    send_msg(ctx, chat_id, &mut message).await?;
                } else {
                    let mut message = Message::new(Viewtype::Text);
                    message.set_text(Some("unsubscription failed".to_owned()));
                    send_msg(ctx, chat_id, &mut message).await?;
                }
            }
            _ => {
                let mut message = Message::new(Viewtype::Text);
                message.set_text(Some(
                    "🕭 Notification-Test-Bot\n\
Sends you messages at random intervals for testing.\n\n\
USAGE:\n\
🔔 /subscribe | /sub  - to subscribe to the bot\n\
🔕 /unsubscribe | /out - to unsubscribe"
                        .to_owned(),
                ));
                send_msg(ctx, chat_id, &mut message).await?;
            }
        }
    }

    Ok(())
}

async fn cb(ctx: &Context, event: EventType, database: &sled::Db) {
    //println!("[{:?}]", event);

    match event {
        EventType::ConfigureProgress { progress, comment } => {
            println!("  progress: {} {:?}", progress, comment);
        }
        EventType::Info(msg) | EventType::Warning(msg) | EventType::Error(msg) => {
            println!(" {}", msg);
        }
        EventType::ConnectivityChanged => {
            println!("ConnectivityChanged: {:?}", ctx.get_connectivity().await);
        }
        EventType::IncomingMsg { chat_id, msg_id } => {
            match handle_message(ctx, chat_id, msg_id, database).await {
                Err(err) => {
                    print!("{}", err);
                }
                Ok(_val) => {}
            }
        }
        ev => {
            println!("[EV] {:?}", ev);
        }
    }
}

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    let dbdir = current_dir()
        .context("failed to get current working directory")?
        .join("deltachat-db");
    let database = sled::open(dbdir.join("bot-data")).expect("open");
    std::fs::create_dir_all(dbdir.clone()).context("failed to create data folder")?;
    let dbfile = dbdir.join("db.sqlite");
    println!("creating database {:?}", dbfile);
    let ctx = Context::new(dbfile.into(), 0)
        .await
        .context("Failed to create context")?;

    let info = ctx.get_info().await;
    println!("info: {:#?}", info);
    let ctx = Arc::new(ctx);

    let events = ctx.get_event_emitter();

    let (interrupt_send, interrupt_recv) = channel::bounded(1);
    ctrlc::set_handler(move || async_std::task::block_on(interrupt_send.send(())).unwrap())
        .context("Error setting Ctrl-C handler")?;

    let is_configured = ctx.get_config_bool(config::Config::Configured).await?;
    if !is_configured {
        println!("configuring");
        if let Some(addr) = vars().find(|key| key.0 == "addr") {
            ctx.set_config(config::Config::Addr, Some(&addr.1))
                .await
                .context("set config failed")?;
        } else {
            panic!("no addr ENV var specified");
        }
        if let Some(pw) = vars().find(|key| key.0 == "mailpw") {
            ctx.set_config(config::Config::MailPw, Some(&pw.1))
                .await
                .context("set config failed")?;
        } else {
            panic!("no mailpw ENV var specified");
        }
        ctx.set_config(config::Config::Bot, Some("1"))
            .await
            .context("set config failed")?;
        ctx.set_config(config::Config::E2eeEnabled, Some("1"))
            .await
            .context("set config failed")?;

        ctx.configure().await.context("configure failed")?;
        println!("configuration done");
    } else {
        println!("account is already configured");
    }

    println!("------ RUN ------");
    ctx.start_io().await;

    let ctx_clone = ctx.clone();
    let db_clone = database.clone();

    let notify_loop = async_std::task::spawn(async move {
        let mut rng = rand::rngs::OsRng;
        loop {
            async_std::task::sleep(Duration::from_secs(rng.gen_range(13..100) * 60)).await;

            for (key, _) in db_clone.iter().filter(|r| r.is_ok()).map(|r| r.unwrap()) {
                let contact_id = ContactId::new(
                    String::from_utf8(key.to_vec())
                        .unwrap()
                        .parse::<u32>()
                        .unwrap(),
                );
                //println!("key {:?}", contact_id);
                let mut message = Message::new(Viewtype::Text);
                let contact_chat_id = ChatId::get_for_contact(&ctx_clone, contact_id)
                    .await
                    .unwrap();

                let now: DateTime<Utc> = Utc::now();

                message.set_text(Some(format!("test message, sent at {:?}", now)));
                send_msg(&ctx_clone, contact_chat_id, &mut message)
                    .await
                    .unwrap();
            }
        }
    });

    // wait for ctrl+c or event
    while let Some(event) = async {
        interrupt_recv.recv().await.unwrap();
        None
    }
    .race(events.recv())
    .await
    {
        cb(&ctx, event.typ, &database).await;
    }

    println!("stopping");
    notify_loop.cancel().await;
    ctx.stop_io().await;
    println!("closing");
    drop(ctx);

    while let Some(event) = events.recv().await {
        println!("ignoring event {:?}", event);
    }

    Ok(())
}
