#[macro_use]
extern crate dotenv_codegen;

use std::sync::Arc;
use serenity::all::{Context, CreateEmbed, CreateEmbedFooter, CreateMessage, EventHandler, GatewayIntents, Message, Ready};
use serenity::{async_trait, Client};
use regex::Regex;
use serenity::builder::CreateAllowedMentions;
use serenity::prelude::TypeMapKey;
use spotify_rs::{ClientCredsClient, ClientCredsFlow};
use spotify_rs::auth::{NoVerifier, Token};
use tokio::sync::RwLock;

struct SpotifyClientHolder;
impl TypeMapKey for SpotifyClientHolder {
    type Value = Arc<RwLock<spotify_rs::client::Client<Token, ClientCredsFlow, NoVerifier>>>;
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {

    async fn message(&self, ctx: Context, msg: Message) {
        let track_re = Regex::new(r"open\.spotify\.com\/track\/([A-Za-z0-9]+)").unwrap();
        let album_re = Regex::new(r"open\.spotify\.com\/album\/([A-Za-z0-9]+)").unwrap();

        let tracks: Vec<spotify_rs::model::track::Track> = {
            let data_read = ctx.data.read().await;
            let spotify_holder_lock = data_read.get::<SpotifyClientHolder>().expect("Expected SpotifyClientHolder in TypeMap").clone();
            let mut spotify_client = spotify_holder_lock.write().await;

            let track_ids = track_re.captures_iter(&*msg.content)
                .take(3) // To prevent heavy load on the spotify api, 10 is Discord's limit
                .map(|c| c.extract())
                .map(|(_, [track_id])| track_id);

            let mut tracks: Vec<spotify_rs::model::track::Track> = vec!();
            for track_id in track_ids {
                if let Ok(track) = spotify_client.track(track_id).market(dotenv!("SPOTIFY_MARKET")).get().await {
                    tracks.push(track);
                }
            };

            tracks
        };

        if tracks.len() > 0 {
            let embeds: Vec<CreateEmbed> = tracks.iter().map(|track| {
                CreateEmbed::new()
                    .title(track.name.clone())
                    .url(track.external_urls.spotify.clone())
                    .thumbnail(
                        if let Some(image) = track.album.images.get(0) {
                            image.url.clone()
                        } else { "".parse().unwrap() }
                    )
                    .field("Album", format!("[{}]({})", track.album.name, track.album.external_urls.spotify), true)
                    .field({
                        if track.artists.len() > 1 { "Artists" } else { "Artist" } },
                           track.artists.iter()
                               .map(|a| format!("[{}]({})", a.name, a.external_urls.spotify))
                               .reduce(|acc, s| format!("{acc}, {s}")).expect("Track missing artists?")
                           ,true)
                    .footer(CreateEmbedFooter::new(format!("Released {}", track.album.release_date.clone())).icon_url(dotenv!("SPOTIFY_ICON_URL")))
            }).collect();

            let builder = CreateMessage::new()
                .embeds(embeds)
                .reference_message(&msg)
                .allowed_mentions(CreateAllowedMentions::new());

            msg.channel_id.send_message(&ctx.http, builder).await.ok();
        }
    }

    async fn ready(&self, _ctx: Context, data_about_bot: Ready) {
        println!("{} is ready!", data_about_bot.user.name)
    }
}

#[tokio::main]
async fn main() {

    let discord_intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    let mut discord_client = Client::builder(dotenv!("DISCORD_TOKEN"), discord_intents).event_handler(Handler).await.expect("Error creating Discord client.");

    let spotify_auth_flow = ClientCredsFlow::new(dotenv!("SPOTIFY_CLIENT_ID"), dotenv!("SPOTIFY_CLIENT_SECRET"));
    let spotify_client = ClientCredsClient::authenticate(spotify_auth_flow).await.expect("Error creating Spotify client");

    {
        let mut data = discord_client.data.write().await;
        data.insert::<SpotifyClientHolder>(Arc::new(RwLock::new(spotify_client)));
    }

    if let Err(why) = discord_client.start().await {
        println!("Client error: {why:?}");
    }
}
