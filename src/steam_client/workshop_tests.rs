use super::*;
use steam_vent_proto::steammessages_publishedfile_steamclient::published_file_details::Child;

fn detail(id: u64, file_type: u32, children: &[u64]) -> PublishedFileDetails {
    let mut d = PublishedFileDetails::new();
    d.set_publishedfileid(id);
    d.set_file_type(file_type);
    for &c in children {
        let mut child = Child::new();
        child.set_publishedfileid(c);
        d.children.push(child);
    }
    d
}

#[test]
fn item_with_children_stays_item() {
    let item = detail_to_workshop_item(&detail(1, 0, &[7, 8]), 570);
    assert_eq!(item.kind, WorkshopItemKind::Item);
    assert_eq!(item.children, vec![7, 8]);
    assert_eq!(item.app_id, 570);
}

#[test]
fn file_type_two_is_collection() {
    let item = detail_to_workshop_item(&detail(2, 2, &[9]), 570);
    assert_eq!(item.kind, WorkshopItemKind::Collection);
    assert_eq!(item.children, vec![9]);
}

#[test]
fn plain_item_without_children() {
    let item = detail_to_workshop_item(&detail(3, 0, &[]), 570);
    assert_eq!(item.kind, WorkshopItemKind::Item);
    assert!(item.children.is_empty());
}

#[test]
fn mod_with_dependencies_stays_in_result() {
    let mut info = HashMap::new();
    info.insert(1, (WorkshopItemKind::Item, vec![7]));
    info.insert(2, (WorkshopItemKind::Collection, vec![5, 6]));
    let (mut visited, mut seen, mut result) = (HashSet::from([1, 2]), HashSet::new(), Vec::new());
    let next = expand_frontier(vec![1, 2], info, &mut visited, &mut seen, &mut result);
    // Mod kept; its deps not pulled.
    assert_eq!(result, vec![1]);
    assert_eq!(next, vec![5, 6]);
}
