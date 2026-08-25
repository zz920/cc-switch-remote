//! 组网鉴权：HMAC 挑战应答 + 时间戳窗口 + nonce 防重放
//!
//! 签名口径：`HMAC-SHA256(share_key, "{ts}|{nonce}|{METHOD}|{path}|{body_sha256_hex}")`
//! 头部格式：`v1:{ts}:{nonce}:{sig_hex}`

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};

type HmacSha256 = Hmac<Sha256>;

/// share id 字符集（去除易混淆字符 0/O、1/I/L）
const SHARE_ID_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// 生成可读 share id：XXXX-XXXX
pub fn generate_share_id() -> String {
    let mut rng = rand::thread_rng();
    use rand::Rng;
    let mut id = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            id.push('-');
        }
        let idx = rng.gen_range(0..SHARE_ID_ALPHABET.len());
        id.push(SHARE_ID_ALPHABET[idx] as char);
    }
    id
}

/// 生成 share key：32 字节随机数，base64url 无填充编码
pub fn generate_share_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_url_encode(&bytes)
}

/// 生成 6 位数字短码（加入审批用）
pub fn generate_short_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32))
}

/// 规范化 share id（去连字符、转大写）
pub fn normalize_share_id(share_id: &str) -> String {
    share_id
        .trim()
        .trim_start_matches("tokentap://join?id=")
        .trim()
        .replace('-', "")
        .to_uppercase()
}

/// share id 命名空间哈希：sha256(规范化 id) 前 16 位 hex
pub fn share_id_hash(share_id: &str) -> String {
    let normalized = normalize_share_id(share_id);
    let digest = Sha256::digest(normalized.as_bytes());
    hex_encode(&digest[..8])
}

/// base64url（无填充）编码
pub fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// base64url 解码（容忍填充/无填充）
pub fn base64_url_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| format!("base64 解码失败: {e}"))
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// 请求体 SHA-256 hex
pub fn body_sha256_hex(body: &[u8]) -> String {
    hex_encode(&Sha256::digest(body))
}

fn sign_message(key: &[u8], message: &str) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|e| format!("HMAC 初始化失败: {e}"))?;
    mac.update(message.as_bytes());
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn build_message(ts: i64, nonce: &str, method: &str, path: &str, body_hash: &str) -> String {
    format!("{ts}|{nonce}|{}|{path}|{body_hash}", method.to_uppercase())
}

/// 生成鉴权头值：`v1:{ts}:{nonce}:{sig}`
pub fn sign_request(
    share_key_b64: &str,
    ts: i64,
    nonce: &str,
    method: &str,
    path: &str,
    body_hash: &str,
) -> Result<String, String> {
    let key = base64_url_decode(share_key_b64)?;
    let sig = sign_message(&key, &build_message(ts, nonce, method, path, body_hash))?;
    Ok(format!("v1:{ts}:{nonce}:{sig}"))
}

/// 校验鉴权头
///
/// 通过条件：格式正确、时间戳在窗口内、nonce 未使用、签名匹配。
#[allow(clippy::too_many_arguments)]
pub fn verify_request(
    share_key_b64: &str,
    header: &str,
    method: &str,
    path: &str,
    body_hash: &str,
    now: i64,
    window_secs: i64,
    nonce_cache: &mut NonceCache,
) -> bool {
    let parts: Vec<&str> = header.splitn(4, ':').collect();
    if parts.len() != 4 || parts[0] != "v1" {
        return false;
    }
    let Ok(ts) = parts[1].parse::<i64>() else {
        return false;
    };
    let nonce = parts[2];
    let sig = parts[3];

    // 时间戳窗口
    if (now - ts).abs() > window_secs {
        return false;
    }
    // 防重放
    if !nonce_cache.check_and_insert(nonce, now, window_secs) {
        return false;
    }

    let Ok(key) = base64_url_decode(share_key_b64) else {
        return false;
    };
    let Ok(expected) = sign_message(&key, &build_message(ts, nonce, method, path, body_hash))
    else {
        return false;
    };
    // 常量时间比较
    constant_time_eq(expected.as_bytes(), sig.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 生成随机 nonce（16 字节 hex）
pub fn generate_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex_encode(&bytes)
}

/// 防重放 nonce 缓存（LRU + 过期清理）
pub struct NonceCache {
    seen: HashMap<String, i64>,
    order: VecDeque<String>,
    capacity: usize,
}

impl NonceCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// 检查并插入 nonce；返回 true = 新鲜（未见过），false = 重放
    pub fn check_and_insert(&mut self, nonce: &str, now: i64, window_secs: i64) -> bool {
        // 惰性过期清理
        if self.seen.len() >= self.capacity / 2 {
            self.evict_expired(now, window_secs);
        }
        if self.seen.contains_key(nonce) {
            return false;
        }
        // 容量满：淘汰最旧
        while self.seen.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            } else {
                break;
            }
        }
        self.seen.insert(nonce.to_string(), now);
        self.order.push_back(nonce.to_string());
        true
    }

    fn evict_expired(&mut self, now: i64, window_secs: i64) {
        let cutoff = now - window_secs * 2;
        while let Some(front) = self.order.front() {
            let expired = self.seen.get(front).map(|ts| *ts < cutoff).unwrap_or(true);
            if expired {
                let n = self.order.pop_front().expect("front checked");
                self.seen.remove(&n);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_id_format_and_hash_stable() {
        let id = generate_share_id();
        assert_eq!(id.len(), 9);
        assert_eq!(id.chars().nth(4), Some('-'));
        // 哈希对大小写/连字符不敏感
        let h1 = share_id_hash("AB3F-K7Q2");
        let h2 = share_id_hash("ab3fk7q2");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let key = generate_share_key();
        let mut cache = NonceCache::new(100);
        let now = 1_700_000_000i64;
        let nonce = generate_nonce();
        let body = br#"{"model":"claude"}"#;
        let hash = body_sha256_hex(body);
        let header = sign_request(&key, now, &nonce, "post", "/v1/messages", &hash).unwrap();

        assert!(verify_request(
            &key,
            &header,
            "POST",
            "/v1/messages",
            &hash,
            now,
            300,
            &mut cache
        ));

        // 重放：同一 nonce 第二次应失败
        assert!(!verify_request(
            &key,
            &header,
            "POST",
            "/v1/messages",
            &hash,
            now,
            300,
            &mut cache
        ));
    }

    #[test]
    fn verify_rejects_wrong_key_and_stale_ts() {
        let key = generate_share_key();
        let wrong_key = generate_share_key();
        let mut cache = NonceCache::new(100);
        let now = 1_700_000_000i64;
        let nonce = generate_nonce();
        let header = sign_request(&key, now, &nonce, "POST", "/v1/messages", "h").unwrap();

        // 错误 key
        assert!(!verify_request(
            &wrong_key,
            &header,
            "POST",
            "/v1/messages",
            "h",
            now,
            300,
            &mut cache
        ));
        // 过期时间戳
        assert!(!verify_request(
            &key,
            &header,
            "POST",
            "/v1/messages",
            "h",
            now + 10_000,
            300,
            &mut cache
        ));
        // 被篡改的路径
        assert!(!verify_request(
            &key,
            &header,
            "POST",
            "/v1/other",
            "h",
            now,
            300,
            &mut cache
        ));
    }

    #[test]
    fn nonce_cache_eviction() {
        let mut cache = NonceCache::new(4);
        let now = 1000i64;
        assert!(cache.check_and_insert("a", now, 300));
        assert!(cache.check_and_insert("b", now, 300));
        assert!(cache.check_and_insert("c", now, 300));
        assert!(cache.check_and_insert("d", now, 300));
        assert!(!cache.check_and_insert("a", now, 300));
        // 插入第 5 个触发淘汰
        assert!(cache.check_and_insert("e", now, 300));
    }
}
