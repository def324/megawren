use anyhow::{Result, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{KeyInit, Tag, XChaCha20Poly1305, XNonce, aead::AeadInOut};
use zeroize::Zeroizing;
const MAGIC: &[u8; 8] = b"MGWSESS\0";
const HEADER: usize = 68;
const LOCKED: [u8; 4] = [1, 0, 1, 1];
// The mode is authenticated along with the rest of the vault header.
const AUTOMATIC: [u8; 4] = [2, 0, 1, 1];
pub fn opens_automatically(data: &[u8]) -> bool {
    data.starts_with(MAGIC) && data.get(8..12) == Some(&AUTOMATIC)
}
fn aad(header: &[u8]) -> Vec<u8> {
    let mut data = b"megawren/session/v1".to_vec();
    data.extend_from_slice(header);
    data
}
fn derive(password: &[u8], header: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let u = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    let (mem, time, lanes) = (u(12), u(16), u(20));
    ensure!(
        (32768..=262144).contains(&mem) && (1..=10).contains(&time) && (1..=4).contains(&lanes),
        "Invalid vault resource limits"
    );
    let params = Params::new(mem, time, lanes, Some(32))
        .map_err(|_| anyhow::anyhow!("Invalid KDF parameters"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, &header[24..40], key.as_mut())
        .map_err(|_| anyhow::anyhow!("Vault key derivation failed"))?;
    Ok(key)
}
pub fn seal(password: &[u8], session: &[u8]) -> Result<Vec<u8>> {
    ensure!(password.len() <= 4096, "Unlock passphrase is too long");
    ensure!(
        !session.is_empty() && session.len() <= 60000,
        "Invalid session size"
    );
    let mut h = vec![0u8; HEADER];
    h[..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(if password.is_empty() {
        &AUTOMATIC
    } else {
        &LOCKED
    });
    for (i, v) in [(12, 65536u32), (16, 3), (20, 1)] {
        h[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }
    getrandom::fill(&mut h[24..64]).map_err(|_| anyhow::anyhow!("System entropy unavailable"))?;
    h[64..68].copy_from_slice(&((session.len() + 16) as u32).to_le_bytes());
    let key = derive(password, &h)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).unwrap();
    let mut payload = Zeroizing::new(session.to_vec());
    let nonce = XNonce::try_from(&h[40..64]).expect("the header holds a 24-byte nonce");
    let tag = cipher
        .encrypt_inout_detached(&nonce, &aad(&h), payload.as_mut_slice().into())
        .map_err(|_| anyhow::anyhow!("Vault encryption failed"))?;
    h.extend_from_slice(&payload);
    h.extend_from_slice(&tag);
    Ok(h)
}
pub fn open(password: &[u8], data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        (HEADER + 16..=65536).contains(&data.len()),
        "Invalid session vault"
    );
    ensure!(
        data.starts_with(MAGIC) && (data[8..12] == LOCKED || opens_automatically(data)),
        "Unsupported session vault"
    );
    ensure!(
        password.is_empty() == opens_automatically(data),
        "Incorrect passphrase for saved login"
    );
    let len = u32::from_le_bytes(data[64..68].try_into().unwrap()) as usize;
    ensure!(len == data.len() - HEADER, "Invalid session vault length");
    ensure!(password.len() <= 4096, "Unlock passphrase is too long");
    let key = derive(password, &data[..HEADER])?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).unwrap();
    let mut plain = Zeroizing::new(data[HEADER..data.len() - 16].to_vec());
    let nonce = XNonce::try_from(&data[40..64]).expect("the header holds a 24-byte nonce");
    let tag = Tag::try_from(&data[data.len() - 16..]).expect("the vault ends with a 16-byte tag");
    if cipher
        .decrypt_inout_detached(
            &nonce,
            &aad(&data[..HEADER]),
            plain.as_mut_slice().into(),
            &tag,
        )
        .is_err()
    {
        bail!("Cannot unlock session: wrong passphrase or damaged vault");
    }
    Ok(plain)
}
#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE_SESSION: &[u8] = b"megawren vault fixture session, no real account";
    #[test]
    fn opens_saved_logins_written_by_earlier_releases() {
        // Written by Megawren 0.2 with argon2 0.5.3 and chacha20poly1305 0.10.1.
        // Dependency upgrades must keep opening existing saved logins.
        let locked = include_bytes!("../tests/fixtures/vault-locked-0.2.bin");
        assert!(!opens_automatically(locked));
        assert_eq!(
            &*open(b"correct horse battery staple", locked).unwrap(),
            FIXTURE_SESSION
        );
        assert!(open(b"correct horse battery stapler", locked).is_err());
        let automatic = include_bytes!("../tests/fixtures/vault-automatic-0.2.bin");
        assert!(opens_automatically(automatic));
        assert_eq!(&*open(b"", automatic).unwrap(), FIXTURE_SESSION);
    }
    #[test]
    fn a_short_passphrase_still_locks_the_session() {
        let data = seal(b"x", b"session bytes").unwrap();
        assert_eq!(&data[8..12], &[1, 0, 1, 1]);
        assert!(open(b"", &data).is_err());
        assert_eq!(&*open(b"x", &data).unwrap(), b"session bytes");
    }
    #[test]
    fn a_passwordless_session_has_a_distinct_authenticated_header() {
        let mut data = seal(b"", b"session bytes").unwrap();
        assert_ne!(&data[8..12], &[1, 0, 1, 1]);
        assert_eq!(&*open(b"", &data).unwrap(), b"session bytes");
        data[8..12].copy_from_slice(&[1, 0, 1, 1]);
        assert!(open(b"", &data).is_err());

        let mut protected = seal(b"x", b"session bytes").unwrap();
        let automatic = seal(b"", b"other").unwrap();
        protected[8..12].copy_from_slice(&automatic[8..12]);
        assert!(open(b"", &protected).is_err());
        assert!(open(b"x", &protected).is_err());
    }
    #[test]
    fn roundtrip_and_tampering() {
        let data = seal(b"test passphrase", b"session bytes").unwrap();
        assert_eq!(&*open(b"test passphrase", &data).unwrap(), b"session bytes");
        assert!(open(b"wrong", &data).is_err());
        let mut bad = data.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(b"test passphrase", &bad).is_err());
        for n in 0..HEADER + 16 {
            assert!(open(b"test", &data[..n.min(data.len())]).is_err());
        }
        let mut bad = data;
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(open(b"test", &bad).is_err());
    }
}
