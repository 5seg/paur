//! Auth helpers: bcrypt password hashing/verification and random
//! session-token generation. Used by both `paur-db` (to persist
//! sessions) and `paur-daemon` (the HTTP login handler).

use rand::RngCore;

/// Length of the random session token, in bytes. 256 bits is what
/// `paur_session` cookies carry; the SHA-256 of this value is what we
/// actually persist.
pub const SESSION_TOKEN_BYTES: usize = 32;

/// Generate a new random session token (raw bytes).
pub fn new_session_token() -> [u8; SESSION_TOKEN_BYTES] {
    let mut buf = [0u8; SESSION_TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// Hash a session token with SHA-256 and return the lower-case hex
/// digest. The DB stores this, never the plaintext token.
pub fn hash_session_token(token: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token);
    hex::encode(digest)
}

/// Hash a plaintext password with bcrypt. The returned string is a
/// self-describing PHC fragment and can be passed straight to
/// [`verify_password`].
pub fn hash_password(plain: &str) -> Result<String, Error> {
    bcrypt::hash(plain, bcrypt::DEFAULT_COST).map_err(|e| Error::Other(format!("bcrypt: {e}")))
}

/// Verify a plaintext password against a stored bcrypt hash. Returns
/// `Ok(true)` on match. An empty stored hash (no admin password set
/// yet) returns `Ok(false)` rather than an error.
pub fn verify_password(plain: &str, stored_hash: &str) -> Result<bool, Error> {
    if stored_hash.is_empty() {
        return Ok(false);
    }
    bcrypt::verify(plain, stored_hash).map_err(|e| Error::Other(format!("bcrypt: {e}")))
}

use crate::Error;
