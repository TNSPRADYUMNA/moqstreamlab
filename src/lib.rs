use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};

pub const MAX_IMAGE: usize = 1_048_576;
pub const HEADER: usize = 52;

pub fn encode(sequence: u64, sent_us: u64, image: &[u8]) -> Result<Vec<u8>> {
    ensure!(!image.is_empty() && image.len() <= MAX_IMAGE, "Image outside allowed size");
    let mut output = Vec::with_capacity(HEADER + image.len());
    output.extend_from_slice(b"MSL1");
    output.extend_from_slice(&sequence.to_be_bytes());
    output.extend_from_slice(&sent_us.to_be_bytes());
    output.extend_from_slice(&Sha256::digest(image));
    output.extend_from_slice(image);
    Ok(output)
}

pub fn decode(frame: &[u8]) -> Result<(u64, u64, &[u8])> {
    ensure!(frame.len() > HEADER && frame.len() <= HEADER + MAX_IMAGE, "Invalid frame length");
    ensure!(&frame[..4] == b"MSL1", "Unknown envelope version");
    let sequence = u64::from_be_bytes(frame[4..12].try_into()?);
    let sent = u64::from_be_bytes(frame[12..20].try_into()?);
    let image = &frame[HEADER..];
    ensure!(Sha256::digest(image).as_slice() == &frame[20..52], "Payload checksum mismatch");
    ensure!(image.starts_with(&[0xff,0xd8]) && image.ends_with(&[0xff,0xd9]), "Expected a complete JPEG");
    Ok((sequence, sent, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_envelope_and_rejects_corruption() {
        let image = [0xff,0xd8,1,2,0xff,0xd9];
        let encoded = encode(17, 100, &image).unwrap();
        assert_eq!(decode(&encoded).unwrap(), (17,100,image.as_slice()));
        for end in 0..HEADER {
            assert!(decode(&encoded[..end]).is_err());
        }
        let mut corrupted = encoded.clone();
        corrupted[HEADER + 2] ^= 1;
        assert!(decode(&corrupted).is_err());
        assert!(encode(0,0,&vec![0;MAX_IMAGE+1]).is_err());
    }
}
