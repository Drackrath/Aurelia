//! Family Sharing borrows and library locks.
//!
//! Steam pushes lock state to us.
use super::*;
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgClientSharedLibraryLockStatus, CMsgClientSharedLibraryStopPlaying,
};

/// Pushed shared-library lock state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryLocks {
    /// Lender account and locking account.
    pub locked: Vec<(u32, u32)>,
    /// Who borrows our library; 0 none.
    pub own_locked_by: u32,
}

/// Last state: (session id, locks).
static LOCKS: std::sync::Mutex<Option<(i32, LibraryLocks)>> = std::sync::Mutex::new(None);

/// Store a pushed lock status.
pub(crate) fn record_lock_status(session_id: i32, msg: &CMsgClientSharedLibraryLockStatus) {
    let locks = LibraryLocks {
        locked: msg
            .locked_library
            .iter()
            .map(|l| (l.owner_id(), l.locked_by()))
            .collect(),
        own_locked_by: msg.own_library_locked_by(),
    };
    tracing::debug!(?locks, "shared library lock status");
    *LOCKS.lock().unwrap_or_else(|e| e.into_inner()) = Some((session_id, locks));
}

/// Lender wants the library back.
pub(crate) fn note_stop_playing(msg: &CMsgClientSharedLibraryStopPlaying) {
    for app in &msg.stop_apps {
        tracing::warn!(
            app_id = app.app_id(),
            lender = app.owner_id(),
            seconds_left = msg.seconds_left(),
            "Family Sharing: the lender is using their library; the borrowed game must stop"
        );
    }
}

/// Account id of a SteamID64.
pub fn account_id(steam_id: u64) -> u32 {
    // Low 32 bits.
    (steam_id & 0xFFFF_FFFF) as u32
}

/// Lender to report, or none.
///
/// Fails closed while someone else plays.
pub fn borrow_owner(
    app_id: u32,
    me: u64,
    owner: Option<u64>,
    locks: Option<&LibraryLocks>,
) -> Result<Option<u32>> {
    let lender = match owner {
        Some(owner) if owner != me => account_id(owner),
        _ => return Ok(None),
    };
    let mine = account_id(me);
    let held = locks.and_then(|l| {
        l.locked
            .iter()
            .find(|(o, by)| *o == lender && *by != 0 && *by != mine)
    });
    if let Some(&(_, by)) = held {
        return Err(LibraryInUse { app_id, lender, by }.into());
    }
    Ok(Some(lender))
}

/// Lender's library held by another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryInUse {
    pub app_id: u32,
    pub lender: u32,
    pub by: u32,
}

impl std::fmt::Display for LibraryInUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "app {} is borrowed through Family Sharing, but the lender's library \
             (account {}) is in use by account {}; Steam lends a library to one player \
             at a time. Try again when they stop playing",
            self.app_id, self.lender, self.by
        )
    }
}

impl std::error::Error for LibraryInUse {}

impl SteamClient {
    /// Lock state for this session.
    pub fn library_locks(&self) -> Option<LibraryLocks> {
        let session = self.connection.as_ref()?.session_id();
        match &*LOCKS.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((s, locks)) if *s == session => Some(locks.clone()),
            _ => None,
        }
    }

    /// Lender account for `app_id`.
    pub async fn lender_for(&self, app_id: u32) -> Result<Option<u32>> {
        let me = self.steam_id().context("not logged in")?;
        let owner = self.app_owner(app_id).await?;
        borrow_owner(app_id, me, owner, self.library_locks().as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_vent_proto::steammessages_clientserver_2::cmsg_client_shared_library_lock_status::LockedLibrary;

    const ME: u64 = 76_561_197_960_287_930;
    const LENDER: u64 = 76_561_197_960_288_000;

    #[test]
    fn own_games_report_no_owner() {
        assert_eq!(borrow_owner(1, ME, Some(ME), None).unwrap(), None);
        assert_eq!(borrow_owner(1, ME, None, None).unwrap(), None);
    }

    #[test]
    fn borrowed_games_name_the_lender() {
        let lender = account_id(LENDER);
        assert_eq!(
            borrow_owner(1, ME, Some(LENDER), None).unwrap(),
            Some(lender)
        );
        // Locked by us: fine.
        let ours = LibraryLocks {
            locked: vec![(lender, account_id(ME))],
            own_locked_by: 0,
        };
        assert_eq!(
            borrow_owner(1, ME, Some(LENDER), Some(&ours)).unwrap(),
            Some(lender)
        );
        // Other lenders' locks: ignored.
        let other = LibraryLocks {
            locked: vec![(lender + 1, 99)],
            own_locked_by: 0,
        };
        assert!(borrow_owner(1, ME, Some(LENDER), Some(&other)).is_ok());
    }

    #[test]
    fn library_in_use_fails_closed() {
        let locks = LibraryLocks {
            locked: vec![(account_id(LENDER), 77)],
            own_locked_by: 0,
        };
        let err = borrow_owner(945_360, ME, Some(LENDER), Some(&locks)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("in use by account 77"), "{msg}");
        let typed = err.downcast_ref::<LibraryInUse>().unwrap();
        assert_eq!((typed.app_id, typed.by), (945_360, 77));
    }

    #[test]
    fn lock_status_is_recorded_per_session() {
        let mut msg = CMsgClientSharedLibraryLockStatus::new();
        let mut lib = LockedLibrary::new();
        lib.set_owner_id(5);
        lib.set_locked_by(6);
        msg.locked_library.push(lib);
        msg.set_own_library_locked_by(7);
        record_lock_status(-42, &msg);
        let stored = LOCKS.lock().unwrap().clone();
        assert_eq!(
            stored,
            Some((
                -42,
                LibraryLocks {
                    locked: vec![(5, 6)],
                    own_locked_by: 7
                }
            ))
        );
        assert_eq!(account_id(LENDER), (LENDER & 0xFFFF_FFFF) as u32);
    }
}
