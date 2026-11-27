//! Ownership from Steam's pushed licence list.

use super::*;
use std::io::{Read, Write};
use std::sync::{LazyLock, Mutex};
use steam_vent::connection::ReadonlyConnection;
use steam_vent_proto::enums_clientserver::EMsg;
use steam_vent_proto::steammessages_clientserver::CMsgClientLicenseList;
use steam_vent_proto_common::{JobMultiple, RpcMessage, RpcMessageWithKind};
use tokio_stream::{Stream, StreamExt};

/// Licence flags that grant nothing.
pub(crate) const INACTIVE_LICENCE_FLAGS: u32 =
    0x04 | 0x08 | 0x10 | 0x20 | 0x400 | 0x800 | 0x2000 | 0x10000;
/// Licence lent by a family member.
pub(crate) const BORROWED_LICENCE_FLAG: u32 = 0x4000;

/// Wait for a session's first push.
const FIRST_PUSH_WAIT: Duration = Duration::from_secs(5);

/// One active licence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Licence {
    pub package_id: u32,
    pub access_token: u64,
    pub change_number: i32,
    /// Lender's account id, if borrowed.
    pub lender: Option<u32>,
}

/// Owned apps, own and borrowed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OwnedApps {
    pub own: HashSet<u32>,
    /// Borrowed apps per lender.
    pub borrowed: HashMap<u32, HashSet<u32>>,
}

impl OwnedApps {
    /// DLC usable with `base`'s licence.
    pub fn owns_dlc(&self, base: u32, dlc: u32) -> bool {
        if self.own.contains(&dlc) {
            return true;
        }
        // Family DLC must share the lender.
        !self.own.contains(&base)
            && self
                .borrowed
                .values()
                .any(|apps| apps.contains(&base) && apps.contains(&dlc))
    }

    /// Group package apps by licence source.
    pub(crate) fn from_packages(
        licences: &[Licence],
        package_apps: &HashMap<u32, Vec<u32>>,
    ) -> Self {
        let mut owned = Self::default();
        for l in licences {
            let Some(apps) = package_apps.get(&l.package_id) else {
                continue;
            };
            match l.lender {
                None => owned.own.extend(apps),
                Some(lender) => owned.borrowed.entry(lender).or_default().extend(apps),
            }
        }
        owned
    }
}

/// Latest licence list of one account.
#[derive(Debug, Clone)]
struct Snapshot {
    licences: Vec<Licence>,
    fingerprint: u64,
}

/// Latest list per SteamID.
static LISTS: LazyLock<Mutex<HashMap<u64, Snapshot>>> = LazyLock::new(Default::default);
/// Fingerprint and the apps it grants.
type OwnedEntry = (u64, Arc<OwnedApps>);

/// Owned apps per SteamID.
static OWNED: LazyLock<Mutex<HashMap<u64, OwnedEntry>>> = LazyLock::new(Default::default);
/// Wakes waiters when a list arrives.
static ARRIVED: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// Active licences of a pushed list.
pub(crate) fn active_licences(msg: &CMsgClientLicenseList) -> Vec<Licence> {
    msg.licenses
        .iter()
        .filter(|l| l.flags() & INACTIVE_LICENCE_FLAGS == 0)
        .map(|l| Licence {
            package_id: l.package_id(),
            access_token: l.access_token(),
            change_number: l.change_number(),
            lender: (l.flags() & BORROWED_LICENCE_FLAG != 0).then(|| l.owner_id()),
        })
        .collect()
}

/// Order-independent FNV-1a licence list hash.
pub(crate) fn fingerprint(licences: &[Licence]) -> u64 {
    let mut keys: Vec<(u32, i32, u32)> = licences
        .iter()
        .map(|l| (l.package_id, l.change_number, l.lender.unwrap_or(0)))
        .collect();
    keys.sort_unstable();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (package, change, lender) in keys {
        for byte in package
            .to_le_bytes()
            .into_iter()
            .chain(change.to_le_bytes())
            .chain(lender.to_le_bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Store a pushed list for `steam_id`.
pub(crate) fn record(steam_id: u64, msg: &CMsgClientLicenseList) {
    if msg.eresult() != 1 {
        tracing::debug!(eresult = msg.eresult(), "ignoring failed licence list");
        return;
    }
    let licences = active_licences(msg);
    let fingerprint = fingerprint(&licences);
    tracing::debug!(
        licences = licences.len(),
        fingerprint,
        "licence list received"
    );
    LISTS.lock().unwrap_or_else(|e| e.into_inner()).insert(
        steam_id,
        Snapshot {
            licences,
            fingerprint,
        },
    );
    ARRIVED.notify_waiters();
}

/// Latest list fingerprint, if any.
pub fn current_fingerprint(steam_id: u64) -> Option<u64> {
    LISTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&steam_id)
        .map(|s| s.fingerprint)
}

/// Record pushes until the session ends.
pub(super) fn watch(mut stream: LicenceStream, steam_id: u64) {
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(msg) => record(steam_id, &msg),
                Err(e) => {
                    tracing::debug!("licence list stream ended: {e}");
                    break;
                }
            }
        }
    });
}

/// Latest list; briefly await first push.
async fn snapshot_for(steam_id: u64) -> Option<Snapshot> {
    let deadline = tokio::time::Instant::now() + FIRST_PUSH_WAIT;
    loop {
        // Register first; no lost wakeups.
        let arrived = ARRIVED.notified();
        if let Some(s) = LISTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&steam_id)
        {
            return Some(s.clone());
        }
        if tokio::time::timeout_at(deadline, arrived).await.is_err() {
            return None;
        }
    }
}

/// Owned apps; `None` without a list.
pub(super) async fn owned_apps(connection: &Connection) -> Result<Option<Arc<OwnedApps>>> {
    let steam_id = u64::from(connection.steam_id());
    let Some(snapshot) = snapshot_for(steam_id).await else {
        return Ok(None);
    };
    if let Some((fp, apps)) = OWNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&steam_id)
        && *fp == snapshot.fingerprint
    {
        return Ok(Some(apps.clone()));
    }
    let packages = package_apps(connection, &snapshot.licences).await?;
    let apps = Arc::new(OwnedApps::from_packages(&snapshot.licences, &packages));
    OWNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(steam_id, (snapshot.fingerprint, apps.clone()));
    Ok(Some(apps))
}

/// One part of a PICS reply.
#[derive(Debug)]
pub(super) struct PicsReplyPart(pub(super) CMsgClientPICSProductInfoResponse);

impl RpcMessage for PicsReplyPart {
    fn parse(reader: &mut dyn Read) -> protobuf::Result<Self> {
        <CMsgClientPICSProductInfoResponse as protobuf::Message>::parse_from_reader(reader)
            .map(Self)
    }
    fn write(&self, writer: &mut dyn Write) -> protobuf::Result<()> {
        self.0.write_to_writer(writer)
    }
    fn encode_size(&self) -> usize {
        self.0.compute_size() as usize
    }
}

impl RpcMessageWithKind for PicsReplyPart {
    type KindEnum = EMsg;
    const KIND: Self::KindEnum = EMsg::k_EMsgClientPICSProductInfoResponse;
}

impl JobMultiple for PicsReplyPart {
    fn completed(&self) -> bool {
        !self.0.response_pending()
    }
}

/// Apps per licensed package.
async fn package_apps(
    connection: &Connection,
    licences: &[Licence],
) -> Result<HashMap<u32, Vec<u32>>> {
    let mut request = CMsgClientPICSProductInfoRequest::new();
    let mut asked = HashSet::new();
    for l in licences {
        // Lenders share package ids.
        if !asked.insert(l.package_id) {
            continue;
        }
        request
            .packages
            .push(cmsg_client_picsproduct_info_request::PackageInfo {
                packageid: Some(l.package_id),
                access_token: Some(l.access_token),
                ..Default::default()
            });
    }
    let mut apps = HashMap::new();
    let mut parts = std::pin::pin!(connection.job_multi::<_, PicsReplyPart>(request));
    while let Some(part) = parts.next().await {
        let part = part.context("failed requesting package info for the licence list")?;
        for package in &part.0.packages {
            apps.insert(package.packageid(), apps_in_package(package.buffer()));
        }
    }
    tracing::debug!(packages = apps.len(), "licence packages resolved");
    Ok(apps)
}

/// App ids in a package buffer.
pub(crate) fn apps_in_package(buffer: &[u8]) -> Vec<u32> {
    // Binary: 4-byte header, then KV.
    let vdf = if pics_buffer_is_text(buffer) {
        find_vdf_in_pics(buffer).ok()
    } else {
        buffer
            .get(4..)
            .and_then(|kv| steam_vdf_parser::parse_binary(kv).ok())
            .map(|v| v.into_owned())
    };
    let Some(vdf) = vdf else {
        return Vec::new();
    };
    let root = vdf.value();
    // `appids` at root or one down.
    let appids = root.get("appids").or_else(|| {
        root.as_obj()
            .and_then(|o| o.values().find_map(|v| v.get("appids")))
    });
    let Some(obj) = appids.and_then(|v| v.as_obj()) else {
        return Vec::new();
    };
    obj.values()
        .filter_map(|v| {
            v.as_i32()
                .and_then(|n| u32::try_from(n).ok())
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .collect()
}

/// Subscribe before logon; catch first push.
pub(super) fn subscribe(
    unauthenticated: &steam_vent::connection::UnAuthenticatedConnection,
) -> LicenceStream {
    Box::pin(unauthenticated.on::<CMsgClientLicenseList>())
}

/// Licence pushes of one session.
pub(super) type LicenceStream = std::pin::Pin<
    Box<
        dyn Stream<Item = std::result::Result<CMsgClientLicenseList, steam_vent::NetworkError>>
            + Send
            + 'static,
    >,
>;
