use crate::{KeyPair, PublicKey, SecretKey, Seed, SharedSecret, Signature, Signer, SignerError};
use cyphernet::crypto::Ecdh;
use ed25519_compact::Error;

#[derive(Debug, Clone)]
pub struct MockSigner {
    pk: PublicKey,
    sk: SecretKey,
}

impl MockSigner {
    pub fn new(rng: &mut fastrand::Rng) -> Self {
        let mut seed: [u8; 32] = [0; 32];

        for byte in &mut seed {
            *byte = rng.u8(..);
        }
        Self::from_seed(seed)
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        let seed = Seed::new(seed);
        let keypair = KeyPair::from_seed(seed);

        Self::from(SecretKey::from(keypair.sk))
    }
}

impl From<SecretKey> for MockSigner {
    fn from(sk: SecretKey) -> Self {
        let pk = sk.public_key().into();
        Self { sk, pk }
    }
}

impl Default for MockSigner {
    fn default() -> Self {
        let seed = Seed::generate();
        let keypair = KeyPair::from_seed(seed);
        let sk = keypair.sk;

        Self {
            pk: sk.public_key().into(),
            sk: sk.into(),
        }
    }
}

impl PartialEq for MockSigner {
    fn eq(&self, other: &Self) -> bool {
        self.pk == other.pk
    }
}

impl Eq for MockSigner {}

impl std::hash::Hash for MockSigner {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pk.hash(state)
    }
}

impl Signer for MockSigner {
    fn public_key(&self) -> &PublicKey {
        &self.pk
    }

    fn sign(&self, msg: &[u8]) -> Signature {
        self.sk.sign(msg, None).into()
    }

    fn try_sign(&self, msg: &[u8]) -> Result<Signature, SignerError> {
        Ok(self.sign(msg))
    }
}

impl Ecdh for MockSigner {
    type Pk = PublicKey;
    type Secret = SharedSecret;
    type Err = Error;

    fn ecdh(&self, _: &Self::Pk) -> Result<Self::Secret, Self::Err> {
        Ok([0u8; 32])
    }
}
