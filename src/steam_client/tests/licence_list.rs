use super::super::licence_list::*;
use steam_vent_proto::steammessages_clientserver::{
    CMsgClientLicenseList, cmsg_client_license_list,
};
use steam_vent_proto::steammessages_clientserver_appinfo::CMsgClientPICSProductInfoResponse;
use steam_vent_proto_common::JobMultiple;

fn licence(package_id: u32, flags: u32, change_number: i32) -> cmsg_client_license_list::License {
    let mut l = cmsg_client_license_list::License::new();
    l.set_owner_id(96573820);
    l.set_package_id(package_id);
    l.set_flags(flags);
    l.set_change_number(change_number);
    l.set_access_token(u64::from(package_id) * 7);
    l
}

fn list(licences: Vec<cmsg_client_license_list::License>) -> CMsgClientLicenseList {
    let mut msg = CMsgClientLicenseList::new();
    msg.set_eresult(1);
    msg.licenses = licences;
    msg
}

#[test]
fn inactive_licences_are_dropped() {
    let msg = list(vec![
        licence(1, 0, 1),
        licence(2, 0x08, 1), // expired
        licence(3, 0x04, 1), // pending
        licence(4, 0x01, 1), // renewing, still active
    ]);
    let ids: Vec<u32> = active_licences(&msg).iter().map(|l| l.package_id).collect();
    assert_eq!(ids, vec![1, 4]);
    assert_eq!(active_licences(&msg)[0].access_token, 7);
}

#[test]
fn fingerprint_ignores_order_but_sees_changes() {
    let a = active_licences(&list(vec![licence(1, 0, 5), licence(2, 0, 6)]));
    let b = active_licences(&list(vec![licence(2, 0, 6), licence(1, 0, 5)]));
    let bought = active_licences(&list(vec![
        licence(1, 0, 5),
        licence(2, 0, 6),
        licence(3, 0, 7),
    ]));
    let updated = active_licences(&list(vec![licence(1, 0, 5), licence(2, 0, 9)]));
    assert_eq!(fingerprint(&a), fingerprint(&b));
    assert_ne!(fingerprint(&a), fingerprint(&bought));
    assert_ne!(fingerprint(&a), fingerprint(&updated));
}

#[test]
fn recorded_list_sets_the_fingerprint() {
    let steam_id = 76_561_190_000_000_001;
    assert_eq!(current_fingerprint(steam_id), None);
    let msg = list(vec![licence(10, 0, 1)]);
    record(steam_id, &msg);
    assert_eq!(
        current_fingerprint(steam_id),
        Some(fingerprint(&active_licences(&msg)))
    );
}

#[test]
fn failed_list_is_ignored() {
    let steam_id = 76_561_190_000_000_002;
    let mut msg = list(vec![licence(10, 0, 1)]);
    msg.set_eresult(2);
    record(steam_id, &msg);
    assert_eq!(current_fingerprint(steam_id), None);
}

#[test]
fn text_package_info_lists_apps() {
    let raw = br#""1749304" { "packageid" "1749304" "appids" { "0" "5045500" "1" "5045501" } }"#;
    let mut apps = apps_in_package(raw);
    apps.sort_unstable();
    assert_eq!(apps, vec![5045500, 5045501]);
}

/// Binary KV after PICS's 4-byte header.
fn binary_package(package_id: u32, apps: &[u32]) -> Vec<u8> {
    let mut out = vec![1, 0, 0, 0];
    let key = |out: &mut Vec<u8>, kind: u8, name: &str| {
        out.push(kind);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    };
    key(&mut out, 0x00, &package_id.to_string());
    key(&mut out, 0x00, "appids");
    for (i, app) in apps.iter().enumerate() {
        key(&mut out, 0x02, &i.to_string());
        out.extend_from_slice(&(*app as i32).to_le_bytes());
    }
    out.extend_from_slice(&[0x08, 0x08, 0x08]);
    out
}

#[test]
fn binary_package_info_lists_apps() {
    let mut apps = apps_in_package(&binary_package(1873594, &[5428010, 2778580]));
    apps.sort_unstable();
    assert_eq!(apps, vec![2778580, 5428010]);
}

#[test]
fn unparseable_package_info_lists_nothing() {
    assert!(apps_in_package(b"\x01\x02garbage").is_empty());
}

#[test]
fn pics_reply_completes_when_nothing_pending() {
    let mut reply = CMsgClientPICSProductInfoResponse::new();
    reply.set_response_pending(true);
    assert!(!PicsReplyPart(reply.clone()).completed());
    reply.set_response_pending(false);
    assert!(PicsReplyPart(reply).completed());
}

#[test]
fn real_pics_package_buffer_lists_its_app() {
    // Vampire Survivors: Legacy of the Moonspell
    let raw = include_bytes!("fixtures/package-799655.bin");
    assert_eq!(apps_in_package(raw), vec![2230760]);
}

#[test]
fn borrowed_licence_names_its_lender() {
    let mut lent = licence(799655, 0x4200, 1);
    lent.set_owner_id(126021521);
    let msg = list(vec![licence(644402, 0x200, 1), lent]);
    let lenders: Vec<Option<u32>> = active_licences(&msg).iter().map(|l| l.lender).collect();
    assert_eq!(lenders, vec![None, Some(126021521)]);
}

fn owned(own: &[u32], borrowed: &[(u32, &[u32])]) -> OwnedApps {
    OwnedApps {
        own: own.iter().copied().collect(),
        borrowed: borrowed
            .iter()
            .map(|(lender, apps)| (*lender, apps.iter().copied().collect()))
            .collect(),
    }
}

#[test]
fn own_dlc_counts_with_any_base() {
    let apps = owned(&[1794680, 2230760], &[]);
    assert!(apps.owns_dlc(1794680, 2230760));
}

#[test]
fn borrowed_dlc_needs_the_base_from_the_same_lender() {
    // Own base; Bloodmoon only lent.
    let apps = owned(&[1794680], &[(126021521, &[1794680, 4781330])]);
    assert!(!apps.owns_dlc(1794680, 4781330));
    // Borrowed base, same lender.
    let apps = owned(&[], &[(126021521, &[1794680, 4781330])]);
    assert!(apps.owns_dlc(1794680, 4781330));
    // Borrowed base, other lender.
    let apps = owned(&[], &[(1, &[1794680]), (2, &[4781330])]);
    assert!(!apps.owns_dlc(1794680, 4781330));
}

#[test]
fn packages_group_by_licence_source() {
    let licences = active_licences(&list(vec![licence(644402, 0, 1), {
        let mut l = licence(1662278, 0x4200, 1);
        l.set_owner_id(126021521);
        l
    }]));
    let packages =
        std::collections::HashMap::from([(644402, vec![1794680]), (1662278, vec![4781330])]);
    let apps = OwnedApps::from_packages(&licences, &packages);
    assert_eq!(apps, owned(&[1794680], &[(126021521, &[4781330])]));
}

#[test]
fn real_multi_app_package_lists_the_base_game() {
    // ELDEN RING package
    let apps = apps_in_package(include_bytes!("fixtures/package-660700.bin"));
    assert!(apps.contains(&1245620), "got {apps:?}");
}
