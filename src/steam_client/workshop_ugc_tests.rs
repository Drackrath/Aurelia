use super::super::workshop_manifest::InstalledWorkshopItem;
use super::*;
use steam_vent_proto::steammessages_publishedfile_steamclient::published_file_details::{
    Tag, VoteData,
};

#[test]
fn details_map_all_fields() {
    let mut d = PublishedFileDetails::new();
    d.set_publishedfileid(42);
    d.set_result(1);
    d.set_creator(7);
    d.set_creator_appid(480);
    d.set_consumer_appid(480);
    d.set_title("Map".into());
    d.set_short_description("short".into());
    d.set_time_created(10);
    d.set_time_updated(20);
    d.set_hcontent_file(999);
    d.set_file_size(1234);
    d.set_preview_url("https://p".into());
    d.set_num_children(2);
    d.set_workshop_accepted(true);
    for t in ["Maps", "", "PvP"] {
        let mut tag = Tag::new();
        tag.set_tag(t.into());
        d.tags.push(tag);
    }
    let mut votes = VoteData::new();
    votes.set_votes_up(5);
    votes.set_votes_down(1);
    votes.set_score(0.75);
    d.vote_data = Some(votes).into();
    let u = details_from_proto(&d);
    assert_eq!((u.id, u.result, u.owner, u.consumer_app), (42, 1, 7, 480));
    assert_eq!((u.title.as_str(), u.description.as_str()), ("Map", "short"));
    assert_eq!(u.tags, "Maps,PvP");
    assert_eq!((u.file, u.file_size, u.children), (999, 1234, 2));
    assert_eq!((u.votes_up, u.votes_down, u.score), (5, 1, 0.75));
    assert!(u.accepted && !u.banned);
    assert_eq!(u.url, "https://p");
    d.set_file_description("long".into());
    assert_eq!(details_from_proto(&d).description, "long");
}

#[test]
fn install_state_compares_manifests() {
    let dir = std::env::temp_dir().join(format!("aurelia-ugc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let record = InstalledWorkshopItem {
        published_file_id: 42,
        manifest_id: 999,
        size: 10,
        time_updated: 20,
    };
    let state = install_state(Some(&record), &dir, Some(999));
    assert!(state.installed && !state.needs_update);
    assert_eq!((state.size, state.timestamp), (10, 20));
    assert_eq!(state.folder, dir.to_string_lossy());
    assert!(install_state(Some(&record), &dir, Some(1000)).needs_update);
    assert!(
        !install_state(Some(&record), &dir, None).needs_update,
        "unknown"
    );
    assert_eq!(install_state(None, &dir, Some(1)), UgcInstall::default());
    let gone = dir.join("missing");
    assert!(!install_state(Some(&record), &gone, None).installed);
    let _ = std::fs::remove_dir_all(&dir);
}
