// headless ticket probe 

use aurelia::steam_client::SteamClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app_id: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(400); // Portal

    let mut client = SteamClient::new()?;
    client.restore_session().await?;
    if !client.is_authenticated() {
        eprintln!("not authenticated — run `aurelia login` first");
        std::process::exit(1);
    }
    println!(
        "session restored headlessly (steam_id {:?}) — no Steam client involved",
        client.steam_id()
    );

    match client.get_app_ticket(app_id).await {
        Ok(t) => println!("ownership ticket for {app_id}: {} bytes (signed, genuine)", t.len()),
        Err(e) => println!("ownership ticket for {app_id}: FAILED — {e}"),
    }
    match client.request_encrypted_app_ticket(app_id).await {
        Ok(t) => println!("encrypted app ticket for {app_id}: {} bytes", t.len()),
        Err(e) => println!("encrypted app ticket for {app_id}: FAILED — {e}"),
    }
    Ok(())
}
