use anyhow::{Result, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce, aead::AeadInPlace};
use zeroize::Zeroizing;
const MAGIC: &[u8; 8] = b"MGWSESS\0";
const HEADER: usize = 68;
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
    ensure!(
        !password.is_empty() && password.len() <= 4096,
        "Invalid unlock passphrase length"
    );
    ensure!(
        !session.is_empty() && session.len() <= 60000,
        "Invalid session size"
    );
    let mut h = vec![0u8; HEADER];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&1u16.to_le_bytes());
    h[10] = 1;
    h[11] = 1;
    for (i, v) in [(12, 65536u32), (16, 3), (20, 1)] {
        h[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }
    getrandom::fill(&mut h[24..64]).map_err(|_| anyhow::anyhow!("System entropy unavailable"))?;
    h[64..68].copy_from_slice(&((session.len() + 16) as u32).to_le_bytes());
    let key = derive(password, &h)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).unwrap();
    let mut payload = Zeroizing::new(session.to_vec());
    let tag = cipher
        .encrypt_in_place_detached(XNonce::from_slice(&h[40..64]), &aad(&h), &mut payload)
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
        &data[..8] == MAGIC && data[8..12] == [1, 0, 1, 1],
        "Unsupported session vault"
    );
    let len = u32::from_le_bytes(data[64..68].try_into().unwrap()) as usize;
    ensure!(len == data.len() - HEADER, "Invalid session vault length");
    ensure!(password.len() <= 4096, "Unlock passphrase is too long");
    let key = derive(password, &data[..HEADER])?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).unwrap();
    let mut plain = Zeroizing::new(data[HEADER..data.len() - 16].to_vec());
    if cipher
        .decrypt_in_place_detached(
            XNonce::from_slice(&data[40..64]),
            &aad(&data[..HEADER]),
            &mut plain,
            chacha20poly1305::Tag::from_slice(&data[data.len() - 16..]),
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
