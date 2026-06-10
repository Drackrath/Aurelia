use reqwest::Client;

use crate::Error;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ContentServer {
    pub r#type: String,
    pub source_id: u64,
    pub cell_id: Option<u32>,
    pub load: u32,
    pub weighted_load: u32,
    pub num_entries_in_client_list: u32,
    pub host: String,
    pub vhost: String,
    pub https_support: String,
    pub priority_class: u32,
}

#[derive(Debug, Deserialize)]
struct ContentServerDirectoryInner {
    pub servers: Vec<ContentServer>,
}

#[derive(Debug, Deserialize)]
struct ContentServerDirectory {
    pub response: ContentServerDirectoryInner,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CDNServer {
    pub r#type: String,
    pub https: bool,
    pub host: String,
    pub vhost: String,
    pub port: u16,
    pub cell_id: u32,
    pub load: u32,
    pub weighted_load: u32,
    pub auth_token: Option<String>,
}

pub async fn get_servers_for_steam_pipe(cell_id: u32) -> Result<Vec<CDNServer>, Error> {
    Ok(
        Client::new()
            .get("https://api.steampowered.com/IContentServerDirectoryService/GetServersForSteamPipe/v1/")
            .query(&[("cell_id", cell_id)])
            .send().await?
            .json::<ContentServerDirectory>().await?
            .response
            .servers
            .into_iter()
            .map(|server| {
                let https = server.https_support == "mandatory";
                CDNServer {
                    r#type: server.r#type,
                    https,
                    host: server.host,
                    vhost: server.vhost,
                    port: if https { 443 } else { 80 },
                    cell_id: server.cell_id.unwrap_or(0),
                    load: server.load,
                    weighted_load: server.weighted_load,
                    auth_token: None,
                }
            })
            .collect::<Vec<CDNServer>>()
    )
}
