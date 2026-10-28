use super::wait_for_launch;
use std::cell::{Cell, RefCell};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(1);

#[tokio::test]
async fn returns_once_the_game_runs() {
    let polls = Cell::new(0);
    let out = wait_for_launch(
        Duration::from_millis(50),
        POLL,
        || {
            polls.set(polls.get() + 1);
            polls.get() >= 3
        },
        || false,
        |_| {},
    )
    .await;
    assert_eq!(out, Ok(()));
}

#[tokio::test]
async fn times_out_when_the_game_never_starts() {
    let out = wait_for_launch(Duration::from_millis(5), POLL, || false, || false, |_| {}).await;
    assert_eq!(out, Err(Duration::from_millis(5)));
}

#[tokio::test]
async fn shader_processing_does_not_use_the_timeout() {
    // Busy far longer than the timeout.
    let polls = Cell::new(0);
    let messages = RefCell::new(Vec::new());
    let out = wait_for_launch(
        Duration::from_millis(3),
        POLL,
        // Starts right after shaders end.
        || polls.get() >= 31,
        || {
            polls.set(polls.get() + 1);
            polls.get() < 30
        },
        |m| messages.borrow_mut().push(m.to_string()),
    )
    .await;
    assert_eq!(out, Ok(()), "shader time must not count");
    let messages = messages.into_inner();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].contains("processing Vulkan shaders"));
    assert!(messages[1].contains("finished"));
}
