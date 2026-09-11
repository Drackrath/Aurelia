//! Current player count over the CM (`ClientGetNumberOfCurrentPlayersDP`).
use super::*;
use std::io::{Read, Write};
use steam_vent_proto::enums_clientserver::EMsg;
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgDPGetNumberOfCurrentPlayers, CMsgDPGetNumberOfCurrentPlayersResponse,
};
use steam_vent_proto_common::{RpcMessage, RpcMessageWithKind};

/// Newtypes give the DP messages their `EMsg` kind.
#[derive(Debug, Default)]
struct PlayersRequest(CMsgDPGetNumberOfCurrentPlayers);

#[derive(Debug, Default)]
struct PlayersResponse(CMsgDPGetNumberOfCurrentPlayersResponse);

macro_rules! kinded {
    ($t:ty, $inner:ty, $kind:expr) => {
        impl RpcMessage for $t {
            fn parse(reader: &mut dyn Read) -> ::protobuf::Result<Self> {
                <$inner as ::protobuf::Message>::parse_from_reader(reader).map(Self)
            }
            fn write(&self, writer: &mut dyn Write) -> ::protobuf::Result<()> {
                use ::protobuf::Message;
                self.0.write_to_writer(writer)
            }
            fn encode_size(&self) -> usize {
                use ::protobuf::Message;
                self.0.compute_size() as usize
            }
        }
        impl RpcMessageWithKind for $t {
            type KindEnum = EMsg;
            const KIND: Self::KindEnum = $kind;
        }
    };
}

kinded!(
    PlayersRequest,
    CMsgDPGetNumberOfCurrentPlayers,
    EMsg::k_EMsgClientGetNumberOfCurrentPlayersDP
);
kinded!(
    PlayersResponse,
    CMsgDPGetNumberOfCurrentPlayersResponse,
    EMsg::k_EMsgClientGetNumberOfCurrentPlayersDPResponse
);

impl SteamClient {
    /// Players in-game right now.
    pub async fn current_players(&self, app_id: u32) -> Result<i32> {
        let connection = self.require_connection()?;
        let mut inner = CMsgDPGetNumberOfCurrentPlayers::new();
        inner.set_appid(app_id);
        let response: PlayersResponse = connection
            .job(PlayersRequest(inner))
            .await
            .context("failed requesting the current player count")?;
        let eresult = response.0.eresult();
        if eresult != 1 {
            bail!("Steam returned EResult {eresult} for the player count of app {app_id}");
        }
        Ok(response.0.player_count())
    }
}
