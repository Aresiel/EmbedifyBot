#[macro_use]
extern crate dotenv_codegen;

use std::sync::Arc;
use std::time::Duration;
use serenity::all::{Context, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage, EventHandler, GatewayIntents, Message, Ready};
use serenity::{async_trait, Client};
use regex::Regex;
use serenity::builder::CreateAllowedMentions;
use serenity::futures::stream;
use serenity::futures::StreamExt;
use serenity::prelude::TypeMapKey;
use spotify_rs::{ClientCredsClient, ClientCredsFlow};
use spotify_rs::auth::{NoVerifier, Token};
use tokio::sync::RwLock;
use tokio::time::sleep;

struct SpotifyClientHolder;
impl TypeMapKey for SpotifyClientHolder {
    type Value = Arc<RwLock<spotify_rs::client::Client<Token, ClientCredsFlow, NoVerifier>>>;
}

struct Handler;

async fn message_author_has_embed_link_perm_inside_guild(ctx: &Context, msg: &Message) -> Option<bool> {
    let guild_channel =  msg.channel(&ctx.http).await.ok()?.guild()?;
    let member = guild_channel.guild_id.member(&ctx.http, msg.author.id).await.ok()?;
    let guild = guild_channel.guild_id.to_partial_guild(&ctx.http).await.ok()?;
    let member_permissions = guild.user_permissions_in(&guild_channel, &member);

    Some(member_permissions.embed_links())
}

fn get_embeddable_spotify_track_ids_in_string(string: &String) -> Vec<String> {
    let track_re = Regex::new(r"(<?)https?://open\.spotify\.com/track/([A-Za-z0-9]*)[&?=A-Za-z0-9]*(>?)").unwrap();

    track_re.captures_iter(&*string)
        .take(3) // To prevent heavy load on the spotify api, 10 is Discord's limit
        .map(|c| c.extract())
        .filter_map(|(_, [left_pad, track_id, right_pad])| {
            if left_pad.len() > 0 && right_pad.len() > 0 { // Handle links with embeds explicitly disabled
                None
            } else {
                Some(track_id.to_string())
            }
        })
        .collect()
}

fn is_spotify_track_id_embedded_in_message(message: &Message, track_id: &String) -> bool {
    message.embeds.iter()
        .filter(|embed| embed.kind.as_ref().is_some_and(|kind| kind == "link"))
        .filter(|embed| embed.provider.as_ref().is_some_and(|provider| provider.name.as_ref().is_some_and(|name| name == "Spotify")))
        .any(|embed| embed.url.as_ref().is_some_and(|url| url.contains(track_id)))
}

#[async_trait]
impl EventHandler for Handler {

    async fn message(&self, ctx: Context, msg: Message) {
        if !message_author_has_embed_link_perm_inside_guild(&ctx, &msg).await.is_some_and(|b| b == true) {
            return
        }

        let tracks: Vec<spotify_rs::model::track::Track> = {
            let track_ids: Vec<String> = get_embeddable_spotify_track_ids_in_string(&msg.content).iter()
                .filter(|track_id| !is_spotify_track_id_embedded_in_message(&msg, track_id)) // Comment out if using embed suppression
                .map(|string| string.clone())
                .collect();


            let data_read = ctx.data.read().await;
            let spotify_holder_lock = data_read.get::<SpotifyClientHolder>().expect("Expected SpotifyClientHolder in TypeMap").clone();
            let mut spotify_client = spotify_holder_lock.write().await;

            let mut tracks: Vec<spotify_rs::model::track::Track> = vec!(); // TODO: Make this less mutating without having all the closures complain :sob:
            for track_id in track_ids {
                if let Ok(track) = spotify_client.track(track_id).market(dotenv!("SPOTIFY_MARKET")).get().await {
                    tracks.push(track);
                }
            };



            /* TODO: Pray for miracles
            let client_data_lock = ctx.data.clone();
            let tracks: Vec<spotify_rs::model::track::Track> = stream::iter(track_ids.iter())
                .filter_map(move |track_id| async move {
                    let client_data_lock = client_data_lock.clone();
                    let client_data = client_data_lock.read().await;
                    let cloned_spotify_holder_lock = client_data.get::<SpotifyClientHolder>().expect("Expected SpotifyClientHolder in TypeMap").clone();
                    let mut spotify_client = cloned_spotify_holder_lock.write().await;
                    spotify_client.track(track_id).market(dotenv!("SPOTIFY_MARKET")).get().await.ok()
                })
                .collect().await;
           */

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

            //sleep(Duration::from_secs(1)).await; // Maybe? Wait to make sure Discord has time to populate the embeds
            //let mut mut_msg = msg.clone();
            //mut_msg.edit(&ctx, EditMessage::new().suppress_embeds(true)).await.ok();
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
