# steam-vent-chat

Steam chat client library.

A high-level companion library for
[steam-vent](https://codeberg.org/steam-vent/steam-vent) for sending and
receiving steam chat messages.

## Usage

```rust
let friend_to_bother: steamid_ng::SteamID = get_steam_id();
let connection: steam_vent::Connection = get_steam_vent_connection();

let chat = ChatClient::new(connection);
chat.send_message(friend_to_bother, "Hey!".into()).await?;

let mut events = chat.listen();
while let Some(Ok(event)) = events.next().await {
    match event {
        ChatEvent::Typing(event) => println!("{} is tying...", event.source.steam64()),
        ChatEvent::Message(event) => println!("{}: {}", event.source.steam64(), event.message_no_bbcode.unwrap_or(event.message)),
        ChatEvent::EchoMessage(event) => println!("me: {}", event.message_no_bbcode.unwrap_or(event.message)),
    }
}
```

See `examples/chat.rs` for a more complete example or
[steam-vent](https://codeberg.org/steam-vent/steam-vent) for more details about
getting a connection.
