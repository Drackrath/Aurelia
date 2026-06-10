use reqwest::{Client, Response};
use std::sync::Arc;
use steam_vent::{
    proto::steammessages_clientserver_appinfo::{
        cmsg_client_picsproduct_info_request::AppInfo, CMsgClientPICSAccessTokenRequest,
        CMsgClientPICSAccessTokenResponse, CMsgClientPICSProductInfoRequest,
        CMsgClientPICSProductInfoResponse,
    },
    Connection, ConnectionTrait,
};

use crate::{web_api::content_service::CDNServer, Error};

use super::depot_chunk;

#[derive(Debug)]
pub(crate) struct InnerClient {
    pub connection: Arc<Connection>,
    web_client: Client,
    pub servers: Vec<CDNServer>,
}

impl InnerClient {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            web_client: Client::new(),
            servers: Vec::new(),
        }
    }

    pub fn cell_id(&self) -> u32 {
        self.connection.cell_id()
    }

    fn get_server(&self) -> Result<&CDNServer, Error> {
        match self
            .servers
            .iter()
            .find(|&server| server.cell_id == self.connection.cell_id())
        {
            Some(server) => Ok(server),
            None => self
                .servers
                .iter()
                .find(|server| server.r#type == "SteamCache" || server.r#type == "CDN")
                .ok_or(Error::Network("no available cdn servers".to_string())),
        }
    }

    pub async fn get_product_info(
        &self,
        app_ids: Vec<u32>,
    ) -> Result<CMsgClientPICSProductInfoResponse, Error> {
        let tokens: CMsgClientPICSAccessTokenResponse = self
            .connection
            .job(CMsgClientPICSAccessTokenRequest {
                appids: app_ids,
                ..Default::default()
            })
            .await?;

        let product_info: CMsgClientPICSProductInfoResponse = self
            .connection
            .job(CMsgClientPICSProductInfoRequest {
                apps: tokens
                    .app_access_tokens
                    .into_iter()
                    .map(|app_token| AppInfo {
                        appid: app_token.appid,
                        access_token: app_token.access_token,
                        ..Default::default()
                    })
                    .collect::<Vec<AppInfo>>(),
                meta_data_only: Some(false),
                ..Default::default()
            })
            .await?;
        Ok(product_info)
    }

    pub async fn remote_cmd<C: AsRef<str>, A: AsRef<str>>(
        &self,
        command: C,
        args: A,
        manifest_request_code: Option<u64>,
    ) -> Result<Response, Error> {
        self.remote_cmd_with_auth(command, args, manifest_request_code, None, None)
            .await
    }

    pub async fn remote_cmd_with_auth<C: AsRef<str>, A: AsRef<str>>(
        &self,
        command: C,
        args: A,
        manifest_request_code: Option<u64>,
        _app_id: Option<u32>,
        _depot_id: Option<u32>,
    ) -> Result<Response, Error> {
        let server = self.get_server()?;
        let mut url = format!(
            "{}://{}:{}/{}/{}",
            if server.https { "https" } else { "http" },
            server.host,
            server.port,
            command.as_ref(),
            args.as_ref()
        );
        if let Some(manifest_request_code) = manifest_request_code {
            url.push('/');
            url.push_str(manifest_request_code.to_string().as_str());
        }

        let mut request = self.web_client.get(&url);

        if let Some(token) = &server.auth_token {
            url.push_str("?token=");
            url.push_str(token);
            request = self.web_client.get(&url).header("x-cdn-auth-token", token);
        }

        let response = request.send().await?;

        Ok(response)
    }

    pub async fn get_chunk(
        &self,
        app_id: u32,
        depot_id: u32,
        depot_key: [u8; 32],
        chunk_id: String,
    ) -> Result<Vec<u8>, Error> {
        let response = self
            .remote_cmd_with_auth(
                "depot",
                format!("{depot_id}/chunk/{chunk_id}"),
                None,
                Some(app_id),
                Some(depot_id),
            )
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::HttpStatus(status));
        }

        let mut bytes = response.bytes().await?.to_vec();
        depot_chunk::decrypt_and_decompress(&mut bytes[..], depot_key).await
    }
}
