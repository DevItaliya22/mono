// SPDX-License-Identifier: MPL-2.0

use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::UpdatePackage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageVerification {
    pub size: u64,
    pub sha256: String,
}

pub fn sha256_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        digest.update(&buffer[..read]);
    }
    Ok((size, lower_hex(&digest.finalize())))
}

pub fn verify_package(
    path: &Path,
    package: &UpdatePackage,
    public_key: &str,
) -> Result<PackageVerification, String> {
    let (size, sha256) = sha256_file(path).map_err(|e| e.to_string())?;
    if size != package.size {
        return Err(format!(
            "downloaded byte count is {size}, expected {}",
            package.size
        ));
    }
    if sha256 != package.sha256 {
        return Err("downloaded package SHA-256 does not match the manifest".into());
    }
    let key = PublicKey::decode(public_key)
        .map_err(|e| format!("embedded Minisign public key is invalid: {e}"))?;
    let signature = Signature::decode(&package.signature)
        .map_err(|e| format!("package Minisign signature is malformed: {e}"))?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|e| format!("could not initialize Minisign verification: {e}"))?;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier
        .finalize()
        .map_err(|e| format!("package Minisign signature is invalid: {e}"))?;
    Ok(PackageVerification { size, sha256 })
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::{KeyPair, sign};
    use std::io::Cursor;

    #[test]
    fn sha256_is_lowercase_and_counts_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"abc").unwrap();
        let (size, hash) = sha256_file(&path).unwrap();
        assert_eq!(size, 3);
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn publisher_signatures_verify_with_the_runtime_verifier() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.zip");
        let bytes = b"signed package bytes";
        std::fs::write(&path, bytes).unwrap();
        let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().unwrap();
        let signature = sign(
            Some(&pk),
            &sk,
            Cursor::new(bytes),
            Some("Koru test"),
            Some("Koru update package"),
        )
        .unwrap()
        .to_string();
        let package = UpdatePackage {
            url: "https://updates.koru.do/releases/test/package.zip".into(),
            file_name: "package.zip".into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            signature,
            format: crate::PackageFormat::Zip,
        };

        verify_package(&path, &package, &pk.to_box().unwrap().to_string()).unwrap();
    }
}
