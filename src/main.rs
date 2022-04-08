extern crate dotenv;
extern crate spacecraft;

mod helpers;

use dotenv::dotenv;
use std::error::Error;
use teloxide::{
    prelude2::*, 
    types::ParseMode, 
    payloads::SendMessageSetters,     
};
use helpers::{DATABASE_URL, twitt_id, TwitterID, get_space};
use spacecraft::{DBManager};


enum Response {
    Text(String),
    None
}

async fn convert_to_tl(url: &str, bot: &AutoSend<Bot>, chat_id: i64) -> Response {
        match twitt_id(url) {
            TwitterID::SpaceId(space_id) => {
                bot.send_message(chat_id, "Your space will be download and send in minutes").await;
                get_space(&space_id, bot, chat_id).await;
            },
            _ => {}
        }
        return Response::None
}


async fn message_handler(
    m: Message,
    bot: AutoSend<Bot>
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let chat = &m.chat;
    let username = chat.username().map(String::from);
    let dbm = DBManager::new(&&DATABASE_URL).unwrap();

    dbm.create_user(
        chat.id, 
        format!("{} {}", chat.first_name().unwrap_or(""), chat.last_name().unwrap_or("")),
        username
    );

    if let Some(maybe_url) = m.text() {
        if maybe_url == "/start" {
            bot.send_message(chat.id, "👉  Send me a valid space url").await?;
        }
        else {
            let response = convert_to_tl(maybe_url, &bot, chat.id).await;

            match response {
                Response::Text(caption) => {
                    bot.send_message(chat.id, caption)
                    .parse_mode(ParseMode::Html)
                    .disable_web_page_preview(true)
                    .await?;
                },
                _ => ()
            }
        }
    }

    Ok(())
}


#[tokio::main]
async fn main() {
    dotenv().ok();

    teloxide::enable_logging!();

    log::info!("Launching SpaceCraft");

    let bot = Bot::from_env().auto_send();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler));

    Dispatcher::builder(bot, handler)
    .default_handler(|_| async {})
    .build()
    .setup_ctrlc_handler().dispatch().await;
}
