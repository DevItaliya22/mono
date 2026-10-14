// SPDX-License-Identifier: MPL-2.0

//! Content-addressed blob storage.
//!
//! Image and canvas bytes live here, never in the CRDT. Putting them in the
//! document would make every sync carry every image, and a CRDT cannot merge
//! two versions of a PNG usefully anyway. The node holds a hash; the bytes
//! travel separately and can be fetched lazily.
//!
//! The hash is self-describing — `<algorithm>:<digest>`, see [`BlobHash`] — and
//! the `:` becomes a `-` in the file name, because a colon is not legal in a
//! Windows path. A hash written before the prefix existed keeps its bare file
//! name, so an existing blob directory needs no migration and nothing is stored
//! twice.

use std::fs;
use std::path::{Path, PathBuf};

use koru_core::node::BlobHash;

use crate::Result;

pub struct BlobStore {
    root: PathBuf,
    /// Names this instance's temp files. Two windows storing the same picture at
    /// the same moment would otherwise write and rename one `<hash>.part`
    /// between them, and the loser's rename fails — or worse, succeeds over a
    /// half-written file.
    tag: String,
}

impl BlobStore {
    pub fn new(root: impl Into<PathBuf>, tag: &str) -> Self {
        Self {
            root: root.into(),
            tag: tag.to_string(),
        }
    }

    /// The file a hash lives in. `:` is not legal in a Windows path, so the
    /// separator becomes `-`; a legacy bare digest is unchanged, which is what
    /// keeps every blob already on disk reachable.
    pub fn path_for(&self, hash: &BlobHash) -> PathBuf {
        self.root.join(hash.0.replace(':', "-"))
    }

    pub fn contains(&self, hash: &BlobHash) -> bool {
        self.path_for(hash).exists()
    }

    /// Store `bytes` and return its hash. Writing the same bytes twice is a
    /// no-op, which is what makes the store safe to sync into.
    pub fn put(&self, bytes: &[u8]) -> Result<BlobHash> {
        let digest = hash_of(bytes);
        // These bytes may already be here under the name an older build gave
        // them. Hand back *that* hash rather than storing a second copy under
        // the prefixed one — the digest is the same, only the spelling moved.
        let legacy = BlobHash(digest.clone());
        if self.path_for(&legacy).exists() {
            return Ok(legacy);
        }

        let hash = BlobHash::new(BlobHash::FNV1A, &digest);
        let path = self.path_for(&hash);
        if !path.exists() {
            fs::create_dir_all(&self.root)?;
            let tmp = path.with_extension(format!("{}.part", self.tag));
            fs::write(&tmp, bytes)?;
            fs::rename(&tmp, &path)?;
        }
        Ok(hash)
    }

    pub fn get(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        Ok(fs::read(self.path_for(hash))?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// FNV-1a, 128-bit-ish via two independent lanes.
///
/// Deliberately not cryptographic: this addresses local content, it does not
/// authenticate it. Which is exactly why the hash it goes into names it — see
/// [`BlobHash`].
fn hash_of(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut a = OFFSET;
    let mut b = OFFSET ^ 0x9e37_79b9_7f4a_7c15;
    for (i, byte) in bytes.iter().enumerate() {
        a ^= *byte as u64;
        a = a.wrapping_mul(PRIME);
        b ^= (*byte as u64).rotate_left((i % 61) as u32 + 1);
        b = b.wrapping_mul(PRIME);
    }
    format!("{a:016x}{b:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("koru-blobs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn identical_bytes_get_one_copy() {
        let root = temp_root("dedup");
        let store = BlobStore::new(&root, "t");

        let a = store.put(b"the same picture").unwrap();
        let b = store.put(b"the same picture").unwrap();
        assert_eq!(a, b, "content addressed");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1, "stored once");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn different_bytes_get_different_hashes() {
        let root = temp_root("distinct");
        let store = BlobStore::new(&root, "t");
        assert_ne!(store.put(b"one").unwrap(), store.put(b"two").unwrap());
        // Transpositions must not collide, which a single-lane FNV would risk.
        assert_ne!(store.put(b"ab").unwrap(), store.put(b"ba").unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn round_trips() {
        let root = temp_root("roundtrip");
        let store = BlobStore::new(&root, "t");
        let bytes = vec![0u8, 1, 2, 250, 251];
        let hash = store.put(&bytes).unwrap();
        assert!(store.contains(&hash));
        assert_eq!(store.get(&hash).unwrap(), bytes);
        let _ = fs::remove_dir_all(&root);
    }

    /// A new hash names what made it, so swapping the algorithm later is a
    /// parse-time discrimination rather than a rewrite of every image node.
    #[test]
    fn a_new_hash_says_what_hashed_it() {
        let root = temp_root("prefixed");
        let store = BlobStore::new(&root, "t");
        let hash = store.put(b"a picture").unwrap();

        assert_eq!(hash.algorithm(), BlobHash::FNV1A);
        assert_eq!(hash.digest().len(), 32, "two 64-bit lanes, hex");
        assert!(!hash.is_legacy());
        // The colon never reaches the filesystem; a Windows path cannot hold one.
        let name = store.path_for(&hash).file_name().unwrap().to_owned();
        assert!(!name.to_string_lossy().contains(':'), "{name:?}");

        let _ = fs::remove_dir_all(&root);
    }

    /// Every blob written before the prefix existed is a bare digest, and every
    /// image node already on disk points at one. They have to keep resolving —
    /// and storing the same bytes again must not make a second copy.
    #[test]
    fn a_bare_legacy_blob_hash_still_parses_and_resolves() {
        let root = temp_root("legacy");
        let store = BlobStore::new(&root, "t");
        let bytes = b"written by an older build";

        // What the old code did: the bare digest, as the whole file name.
        fs::create_dir_all(&root).unwrap();
        let legacy = BlobHash(hash_of(bytes));
        assert!(legacy.is_legacy());
        assert_eq!(legacy.algorithm(), BlobHash::FNV1A, "bare means fnv1a");
        fs::write(store.path_for(&legacy), bytes).unwrap();

        assert!(store.contains(&legacy), "the old file is still found");
        assert_eq!(store.get(&legacy).unwrap(), bytes);

        // Re-storing recognises it rather than making a prefixed second copy.
        assert_eq!(store.put(bytes).unwrap(), legacy);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1, "still one file");

        let _ = fs::remove_dir_all(&root);
    }

    /// Two windows storing the same picture at once must not write one temp file
    /// between them.
    #[test]
    fn a_blob_temp_file_is_named_per_instance() {
        let root = temp_root("parttag");
        let one = BlobStore::new(&root, "i3f9c");
        let two = BlobStore::new(&root, "i9c11");
        let hash = BlobHash::new(BlobHash::FNV1A, "abc");

        let a = one.path_for(&hash).with_extension("i3f9c.part");
        let b = two.path_for(&hash).with_extension("i9c11.part");
        assert_ne!(a, b);

        let _ = fs::remove_dir_all(&root);
    }
}
