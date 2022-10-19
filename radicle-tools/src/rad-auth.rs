fn main() -> anyhow::Result<()> {
    let profile = if let Ok(v) = radicle::Profile::load() {
        v
    } else {
        let keypair = radicle::crypto::KeyPair::generate();
        radicle::ssh::agent::register(&keypair.sk)?;
        radicle::Profile::init(keypair)?
    };
    println!("id: {}", profile.id());
    println!("home: {}", profile.home.display());

    Ok(())
}
