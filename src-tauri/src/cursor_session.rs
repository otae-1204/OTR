//! Cursor 的 `cursorAuth/accessToken`。
//!
//! 旧版是明文 JWT。现在写成 `cursor-safe-storage:<版本>:<base64>`,里面是
//! Chromium OSCrypt 的 `v10` 密文。解开后仍是原来的 JWT,cookie 拼法不变。
//! 明文只留在内存里,不写回 `state.vscdb`。
//!
//! Windows:`Local State` 的 `os_crypt.encrypted_key`(去掉 `DPAPI` 头再用
//! DPAPI 解开)是 32 字节 AES 密钥,密文是 `v10` + 12 字节 nonce + AES-256-GCM。
//! macOS:钥匙串条目 `Cursor Safe Storage` / `Cursor`,PBKDF2-HMAC-SHA1
//! (salt `saltysalt`,1003 轮)得到 16 字节密钥,`v10` 后面是 AES-128-CBC。

use std::path::Path;

use base64::Engine;
use rusqlite::{Connection, OpenFlags};

const WRAPPER: &str = "cursor-safe-storage:";
#[cfg(target_os = "macos")]
const MAC_SERVICE: &str = "Cursor Safe Storage";
#[cfg(target_os = "macos")]
const MAC_ACCOUNT: &str = "Cursor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessToken {
    /// 库不在,或没有 `cursorAuth/accessToken`
    Missing,
    /// 明文 JWT,或已经解开的 JWT
    Ready(String),
    /// 有密文,但这台机器解不开
    Sealed,
}

pub fn read_access_token(user_dir: &Path) -> AccessToken {
    let db = user_dir
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    if !db.is_file() {
        return AccessToken::Missing;
    }
    let Some(conn) = open_ro(&db) else {
        return AccessToken::Missing;
    };
    let raw: String = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
        [],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(_) => return AccessToken::Missing,
    };
    unwrap_token(&raw, |blob| decrypt_blob(blob, user_dir))
}

fn unwrap_token(raw: &str, opener: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> AccessToken {
    match classify(raw) {
        Stored::Empty => AccessToken::Missing,
        Stored::Plain(text) => AccessToken::Ready(text),
        Stored::Broken => AccessToken::Sealed,
        Stored::Wrapped(blob) => match opener(&blob).and_then(accept_plain) {
            Some(text) => AccessToken::Ready(text),
            None => AccessToken::Sealed,
        },
    }
}

enum Stored {
    Empty,
    Plain(String),
    Wrapped(Vec<u8>),
    Broken,
}

fn classify(raw: &str) -> Stored {
    let text = stored_text(raw);
    if text.is_empty() {
        return Stored::Empty;
    }
    let Some(rest) = text.strip_prefix(WRAPPER) else {
        return Stored::Plain(text);
    };
    let Some((_, b64)) = rest.split_once(':') else {
        return Stored::Broken;
    };
    match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
        Ok(blob) if blob.starts_with(b"v10") => Stored::Wrapped(blob),
        _ => Stored::Broken,
    }
}

fn stored_text(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('"') {
        if inner.ends_with('"') {
            if let Ok(text) = serde_json::from_str::<String>(raw) {
                return text;
            }
        }
    }
    raw.to_string()
}

fn accept_plain(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8(bytes).ok()?;
    let text = text.trim();
    if text.is_empty() || text.chars().any(|c| c == '\0') {
        None
    } else {
        Some(text.to_string())
    }
}

fn open_ro(path: &Path) -> Option<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(conn) = Connection::open_with_flags(path, flags) {
        return Some(conn);
    }
    let uri = format!(
        "file:{}?mode=ro&immutable=1",
        path.to_string_lossy().replace('\\', "/")
    );
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn decrypt_blob(blob: &[u8], user_dir: &Path) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let key = windows_key(user_dir)?;
        return decrypt_windows_v10(blob, &key);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = user_dir;
        let secret = macos_secret()?;
        return decrypt_macos_v10(blob, &secret);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (blob, user_dir);
        None
    }
}

/// `v10` + 12 字节 nonce + 密文(含 16 字节 GCM tag)
fn decrypt_windows_v10(blob: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    let rest = blob.strip_prefix(b"v10")?;
    if rest.len() < 12 + 16 {
        return None;
    }
    let (nonce, ct) = rest.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(nonce), ct).ok()
}

/// Chromium macOS OSCrypt:`v10` 之后是 AES-128-CBC,IV 为 16 个空格。
#[cfg(any(test, target_os = "macos"))]
fn decrypt_macos_v10(blob: &[u8], secret: &[u8]) -> Option<Vec<u8>> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

    let ct = blob.strip_prefix(b"v10")?;
    if ct.is_empty() || ct.len() % 16 != 0 {
        return None;
    }
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(secret, b"saltysalt", 1003, &mut key);
    let iv = [b' '; 16];
    let mut buf = ct.to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv)
        .ok()?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?
        .to_vec();
    Some(plain)
}

#[cfg(windows)]
fn windows_key(user_dir: &Path) -> Option<[u8; 32]> {
    let text = std::fs::read_to_string(user_dir.join("Local State")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let b64 = v.pointer("/os_crypt/encrypted_key")?.as_str()?;
    let blob = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let protected = blob.strip_prefix(b"DPAPI")?;
    let key = unprotect_dpapi(protected)?;
    key.try_into().ok()
}

#[cfg(windows)]
fn unprotect_dpapi(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    if data.is_empty() || data.len() > u32::MAX as usize {
        return None;
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // CRYPTPROTECT_UI_FORBIDDEN:解密失败就返回,不弹系统对话框
    const UI_FORBIDDEN: u32 = 0x1;
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() || output.cbData == 0 {
        if !output.pbData.is_null() {
            unsafe {
                LocalFree(output.pbData as _);
            }
        }
        return None;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as _);
    }
    Some(bytes)
}

#[cfg(target_os = "macos")]
fn macos_secret() -> Option<Vec<u8>> {
    let entry = keyring::Entry::new(MAC_SERVICE, MAC_ACCOUNT).ok()?;
    let secret = entry.get_password().ok()?;
    if secret.is_empty() {
        None
    } else {
        Some(secret.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = "eyJhbGciOiJub25lIn0.e30.sig";
    /// key = 0..32, nonce = 0..12, 明文是上面的 PLAIN。Python `AESGCM` 生成。
    const WIN_BLOB: &str = "djEwAAECAwQFBgcICQoLInucc6eioXLCKN3+09tNAcq4txqVSG9SSw6C3wlPMQIkMbJwAGmhLlKv6g==";
    /// secret = "peanuts",同一段明文。Chromium macOS 的 PBKDF2 + AES-CBC。
    const MAC_BLOB: &str = "djEwqPuLJc1rfU5oMAZrXr+tZhucVynwwoXofydF96mbvsE=";

    #[test]
    fn plaintext_jwt_is_used_as_is() {
        let got = unwrap_token(PLAIN, |_| None);
        assert_eq!(got, AccessToken::Ready(PLAIN.into()));
    }

    #[test]
    fn json_quoted_plaintext_is_unwrapped() {
        let raw = serde_json::to_string(PLAIN).unwrap();
        let got = unwrap_token(&raw, |_| None);
        assert_eq!(got, AccessToken::Ready(PLAIN.into()));
    }

    #[test]
    fn empty_value_is_missing() {
        assert_eq!(unwrap_token("  ", |_| None), AccessToken::Missing);
    }

    #[test]
    fn windows_v10_blob_decrypts_to_the_jwt() {
        let b64 = base64::engine::general_purpose::STANDARD
            .decode(WIN_BLOB)
            .unwrap();
        let wrapped = format!("cursor-safe-storage:v1:{}", WIN_BLOB);
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let got = unwrap_token(&wrapped, |blob| {
            assert_eq!(blob, b64.as_slice());
            decrypt_windows_v10(blob, &key)
        });
        assert_eq!(got, AccessToken::Ready(PLAIN.into()));
    }

    #[test]
    fn macos_v10_blob_decrypts_with_the_keychain_secret() {
        let blob = base64::engine::general_purpose::STANDARD
            .decode(MAC_BLOB)
            .unwrap();
        let plain = decrypt_macos_v10(&blob, b"peanuts").unwrap();
        assert_eq!(plain, PLAIN.as_bytes());
    }

    #[test]
    fn wrong_key_stays_sealed() {
        let wrapped = format!("cursor-safe-storage:v1:{}", WIN_BLOB);
        let got = unwrap_token(&wrapped, |_| None);
        assert_eq!(got, AccessToken::Sealed);
    }

    #[test]
    fn broken_wrapper_stays_sealed() {
        assert_eq!(
            unwrap_token("cursor-safe-storage:v1:not-base64!!!", |_| None),
            AccessToken::Sealed
        );
    }

    /// 本机装了 Cursor 且登录态已加密时,必须能解开。没装则跳过。
    #[test]
    fn installed_cursor_session_unwraps_when_present() {
        let dir = crate::paths::cursor_user_dir();
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        if !db.is_file() {
            return;
        }
        match read_access_token(&dir) {
            AccessToken::Missing => {}
            AccessToken::Ready(token) => {
                assert!(
                    !token.starts_with(WRAPPER),
                    "加密的登录态被原样当成了 JWT"
                );
            }
            AccessToken::Sealed => {
                if cfg!(windows) && dir.join("Local State").is_file() {
                    panic!("Cursor 登录态是密文,但本机 DPAPI 没有解开");
                }
            }
        }
    }
}
