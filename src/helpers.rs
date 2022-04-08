extern crate lazy_static;
extern crate m3u8_rs;

use reqwest::Client;
use teloxide::{
    adaptors::AutoSend, 
    prelude2::Bot, 
    prelude::Requester, 
    types::InputFile, 
};
use tokio::task::JoinHandle;
use std::env;
use m3u8_rs::playlist::{Playlist, MediaSegment};
use spacecraft::serde_schemes::*;
use bytes::{self, BufMut, Buf};
use tokio::time::{sleep, Duration};


lazy_static::lazy_static! {
    static ref TWITTER_GUEST_TOKEN_URL: &'static str = "https://api.twitter.com/1.1/guest/activate.json";
    static ref TWITTER_GUEST_BEARER_TOKEN: String = format!("Bearer {}", env::var("TWITTER_GUEST_BEARER_TOKEN").unwrap());
    static ref TWITTER_AUDIO_SPACE_URL: String = format!(
        "https://twitter.com/i/api/graphql/{}/AudioSpaceById?variables=\
        %7B%22id%22%3A%22{{ID}}%22%2C%22isMetatagsQuery%22%3Afalse%2C%22withSuperFollowsUserFields%22%3Afalse%2C\
        %22withDownvotePerspective%22%3Afalse%2C%22withReactionsMetadata%22%3Afalse%2C%22withReactionsPerspective%22%3Afalse%2C\
        %20%22withSuperFollowsTweetFields%22%3Afalse%2C%22withReplays%22%3Afalse%2C\
        %22__fs_dont_mention_me_view_api_enabled%22%3Afalse%2C%22__fs_interactive_text_enabled%22%3Afalse%2C\
        %22__fs_responsive_web_uc_gql_enabled%22%3Afalse%7D
        ",
        env::var("GRAPHQL_PATH").unwrap()
    );
    static ref TWITTER_SPACE_METADATA_URL: &'static str = "https://twitter.com/i/api/1.1/live_video_stream/status/{MEDIA_KEY}?client=web&use_syndication_guest_id=false&cookie_set_host=twitter.com";
    pub static ref DATABASE_URL: String = env::var("DATABASE_URL").unwrap();
}

const CHUNCK: usize = 20;

const BOT_MAX_FILIE_SIZE: usize = 50 * 1000000; // 50 MB


pub enum TwitterID {
    SpaceId(String),
    None
}


pub fn twitt_id(link: &str) -> TwitterID {
    if link.starts_with("https://twitter.com/i/spaces/") {
        let splited: Vec<&str> = (&link[29..]).split("?").collect();
        if splited.len() > 0 {
            return TwitterID::SpaceId(splited[0].to_string())
        }
    }
    TwitterID::None
}

async fn guest_token(client: &Client) -> Option<String> {
    let resp = client.post(*TWITTER_GUEST_TOKEN_URL)
                     .header("AUTHORIZATION", &*TWITTER_GUEST_BEARER_TOKEN)
                     .send()
                     .await;
    if let Ok(response) = resp {
        if response.status().as_u16() != 200 {
            return None;
        }

        if let Ok(token) = response.json::<GuestToken>().await {
            return  Some(token.guest_token);
        }
    }
    return None;
}

fn media_key_url(id: &str) -> String {
    return TWITTER_AUDIO_SPACE_URL.replace("{ID}", id)
}

fn metadata_url(media_key: &str) -> String {
    return TWITTER_SPACE_METADATA_URL.replace("{MEDIA_KEY}", media_key)
}

async fn space_media_key(client: &Client, space_id: &str) -> Option<SpaceMetadata> {
    if let Some(token) = guest_token(&client).await {
        let resp = client.get(&media_key_url(space_id))
                     .header("x-guest-token", &token)
                     .header("Authorization", &*TWITTER_GUEST_BEARER_TOKEN)
                     .send()
                     .await;
        
        if let Ok(response) = resp {
            if response.status().as_u16() != 200 {
                return None;
            }
            if let Ok(mut obj) = response.json::<SpaceObject>().await {
                obj.data.audioSpace.metadata.token = Some(token);
                return Some(obj.data.audioSpace.metadata);
            }
        }
    }
    return None
}

async fn space_playlist(client: &Client, space_id: &str) -> Option<String> {
    if let Some(space_obj) = space_media_key(client, space_id).await {
        let resp = client.get(metadata_url(&space_obj.media_key))
                     .header("AUTHORIZATION", &*TWITTER_GUEST_BEARER_TOKEN)
                     .header("X-Guest-Token", space_obj.token.unwrap())
                     .send()
                     .await;
        if let Ok(response) = resp {
            if response.status().as_u16() != 200 {
                return None;
            }
            let data = response.json::<SpacePlaylist>().await.unwrap();

            return Some(data.source.location)
        }
    }
    return None
}

async fn download_playlist(client: &Client, location: &str) -> Option<Vec<MediaSegment>>{
    let resp = client.get(location)
    .send()
    .await;

    if let Ok(response) = resp {
        if response.status().as_u16() != 200 {
            return None;
        }
        let bytes = response.bytes().await.unwrap();

        match m3u8_rs::parse_playlist_res(&bytes) {
            Ok(Playlist::MediaPlaylist(pl)) => {
                return Some(pl.segments)
            },
            _ => return None
        }
    
    }
    None
}

async fn get_acc_file(uri: String) -> bytes::Bytes {
    loop {
        if let Ok(response) = reqwest::get(&uri).await {
            if response.status().as_u16() != 200 {
                continue;
            }

            if let Ok(_bytes) = response.bytes().await {
                return _bytes
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn download(pl_location: String, bot: AutoSend<Bot>, chat_id: i64, space_id: String) {
    let client = reqwest::Client::new();
    
    if let Some(segments) = download_playlist(&client, &pl_location).await {
        let index = pl_location.rfind("/").unwrap();
        let base_location = &pl_location[..index];
        
        let mut joins: Vec<JoinHandle<bytes::Bytes>> = Vec::with_capacity(CHUNCK);
        let mut input_files:  Vec<InputFile> = Vec::new();
        let mut buf = bytes::BytesMut::with_capacity(BOT_MAX_FILIE_SIZE);
        let mut chunk_number = 1;
        let mut part = 1;

        for chunck in segments.chunks(CHUNCK) {
            for segment in chunck {
                let address = format!("{}/{}", base_location, &segment.uri);
                let join = tokio::task::spawn(async {
                    get_acc_file(address).await
                });
                joins.push(join);
            }

            /* polling threads to detect if they are finished
             * 
             * TODO: implement a pushing logic to execute a callback when all threads are finished
             */
            for join in &mut joins {
                if let Ok(bytes_) = join.await {
                    if buf.remaining() + bytes_.len() > BOT_MAX_FILIE_SIZE {
                        log::info!("[{}] part {} is full - length: {} - partitioning...", &space_id, part, buf.len());
                        
                        input_files.push(
                            InputFile::memory(buf.copy_to_bytes(buf.len()))
                            .file_name(
                                format!("{}-part{}", &space_id, part)
                            )
                        );

                        buf.clear();    // don't really need this after calling copy_to_bytes
                        part += 1;
                    } else {
                        buf.put(bytes_);
                    }
                }
            }

            joins.clear();

            log::info!("[{}] chunk {} is downloaded", &space_id, chunk_number);
            
            chunk_number+=1;

            sleep(Duration::from_millis(100)).await;
        }        

        log::info!("[{}] partitioning last part... - length: {}", &space_id, buf.len());

        // create input file for last part
        input_files.push(
            InputFile::memory(buf.copy_to_bytes(buf.len()))
            .file_name(
                format!("{}-part{}", &space_id, part)
            )
        );
        
        buf.clear();

        log::info!("[{}] Download finished - number of parts: {}", &space_id, part);


        for input_file in input_files {
            // loop {
                let res = bot.send_audio(chat_id, input_file).await;
    
                if res.is_ok() {
                    break
                }

                log::error!("[{}] send failed: {:?}", &space_id, res);
    
                // sleep(Duration::from_millis(100)).await;
            // }
        }
    }
}

pub async fn get_space(space_id: &str, bot: &AutoSend<Bot>, chat_id: i64) {
    let client = reqwest::Client::new();
    if let Some(pl_location) = space_playlist(&client, space_id).await {
        let bot_copy = bot.clone();
        let sid = space_id.to_string();
        
        tokio::task::spawn(async move {
            download(pl_location, bot_copy, chat_id, sid).await;
        });
    } else {
        bot.send_message(chat_id, "Can't download space. Please try later").await;
    }
}
